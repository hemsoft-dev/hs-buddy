"""Actual new consumers, fake native APIs only. No native delivery proof."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch
from mac_process_receive_fixtures import Boundary

spec = importlib.util.spec_from_file_location("receive", Path(__file__).with_name("macos-process-receive.py"))
receive = importlib.util.module_from_spec(spec)
spec.loader.exec_module(receive)


class API:
    def __init__(self):
        self.removed = False
        self.woken = False
        self.callback = None
        self.tick = threading.Event()

    def install(self, pid, callback):
        self.callback = callback
        return object()

    def read(self, resource):
        self.tick.wait(0.002)
        return True

    def wake(self, resource):
        self.woken = True

    def remove(self, resource):
        self.removed = True

    def timestamp(self, event):
        return 123


class FixtureCase(unittest.TestCase):
    def setUp(self):
        self.boundary = Boundary(receive)
        self.addCleanup(self.boundary.close)


class ReceiveTests(FixtureCase):
    def test_guard_refuses_before_native_loading_or_thread(self):
        with patch.object(receive.sys, "platform", "linux"), patch.object(receive, "Native") as api:
            with self.assertRaises(RuntimeError):
                receive.Observer(None, "absent", api_factory=api)
            api.assert_not_called()

    def test_real_reader_passthrough_privacy_and_quiescence(self):
        api = API()
        with subprocess.Popen([sys.executable, "-c", "import sys;sys.stdin.buffer.read()"], stdin=subprocess.PIPE) as process:
            with patch.object(receive, "target_guard"):
                observer = receive.Observer(process, sys.executable, native=True, api_factory=lambda: api)
            self.assertTrue(observer.start(time.monotonic() + 1))
            pointer = 0x123456
            self.assertEqual(api.callback(None, 1, pointer, None), pointer)
            self.assertEqual(set(observer.rows[0]), {"phase", "kind", "timestamp", "wallMillis", "sequence"})
            self.assertEqual(observer.rows[0]["timestamp"], 123)
            observer.stop(time.monotonic() + 1)
            self.assertTrue(api.removed and api.woken)
            self.assertFalse(observer.thread.is_alive())
            before = list(observer.rows)
            self.assertEqual(api.callback(None, 1, pointer, None), pointer)
            self.assertEqual(observer.rows, before)
            process.stdin.close()
            process.stdin = None
            process.wait(timeout=1)

    def test_disabled_overflow_and_getter_failure_cannot_be_qualified(self):
        for failure in ("disabled", "overflow", "getter"):
            with self.subTest(failure=failure), patch.object(receive, "target_guard"):
                process = unittest.mock.Mock()
                process.poll.return_value = None
                api = API()
                value = receive.Observer(process, "synthetic", api_factory=lambda: api)
                value.api, value.active, value.admitted = api, True, True
                kind = 0xFFFFFFFE if failure == "disabled" else 1
                if failure == "overflow":
                    value.rows = [{}] * receive.LIMIT
                if failure == "getter":
                    api.timestamp = unittest.mock.Mock(side_effect=RuntimeError("private"))
                self.assertEqual(value.callback(None, kind, 999, None), 999)
                self.assertFalse(value.active)
                self.assertFalse(value.receipt()["coverageQualified"])
                self.assertNotIn("private", json.dumps(value.receipt()))

    def test_unavailable_native_api_still_quiesces(self):
        with patch.object(receive, "target_guard"):
            process = unittest.mock.Mock()
            process.poll.return_value = None
            api = API()
            api.install = lambda *_: None
            observer = receive.Observer(process, "synthetic", api_factory=lambda: api)
            self.assertFalse(observer.start(time.monotonic() + 1))
            result = observer.stop(time.monotonic() + 1)
            self.assertEqual(result["reason"], "unavailable")
            self.assertTrue(result["stopped"])

    def test_partial_oversized_wrong_phase_eof_and_deadline(self):
        for payload in (b"rea", b"x" * 65, b"wrong\n", b"ready\nextra", b""):
            with self.subTest(payload=payload):
                r, w = receive.os.pipe()
                try:
                    if payload:
                        receive.os.write(w, payload)
                    receive.os.close(w)
                    channel = receive.Channel(r, r, time.monotonic() + 0.05)
                    with self.assertRaises((RuntimeError, TimeoutError)):
                        channel.expect("ready")
                finally:
                    receive.os.close(r)
        r, w = receive.os.pipe()
        try:
            channel = receive.Channel(r, w, time.monotonic() + 0.01)
            with self.assertRaises(TimeoutError):
                channel.expect("ready")
        finally:
            receive.os.close(r); receive.os.close(w)

    def test_actual_controller_popen_protocol_and_independent_functional_result(self):
        # This executable models only the native child's pipe contract.
        source = """#!/usr/bin/env python3
import os
r=int(os.environ['BUDDY_RECEIVE_READ']); w=int(os.environ['BUDDY_RECEIVE_WRITE'])
def put(x): os.write(w,(x+'\\n').encode())
def get():
 b=b''
 while not b.endswith(b'\\n'):
  x=os.read(r,1)
  if not x: raise SystemExit(2)
  b+=x
 return b.strip()
put('ready')
assert get() in (b'attached',b'unavailable')
put('dispatched')
assert get()==b'quiesced'
put('completed')
"""
        class Unavailable:
            def __init__(self, *args, **kwargs):
                self.finished = threading.Event()
            def start(self, deadline):
                return False
            def stop(self, deadline):
                self.finished.set()
                return dict(reason="unavailable", stopped=True, received=[], coverageQualified=False)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            exe = root / "native-consumer"
            exe.write_text(source)
            exe.chmod(0o700)
            self.boundary.register_script(exe)
            output = root / "receipt.json"
            env = {"BUDDY_RECEIVER_CONTROL_EXECUTABLE": str(exe)}
            with patch.object(receive, "canonical_guard"), patch.dict(os.environ, env):
                result = receive.native_control(exe, output, observer_factory=Unavailable)
            self.assertTrue(result["functional"])
            self.assertEqual(result["phases"], ["started", "ready", "unavailable",
                "dispatched", "quiesced", "completed"])
            self.assertEqual(result["diagnostic"], "unqualified")
            self.assertFalse(result["coverageQualified"])
            # Mutating the actual child's phase fails, retains a false result,
            # and controller finally closes pipes/reaps its OWN process.
            exe.write_text(source.replace("put('dispatched')", "put('unexpected')"))
            output = root / "failure-receipt.json"
            with patch.object(receive, "canonical_guard"), patch.dict(os.environ, env):
                with self.assertRaises(RuntimeError):
                    receive.native_control(exe, output, observer_factory=Unavailable)
            failed = json.loads(output.read_text())
            self.assertFalse(failed["functional"])
            self.assertEqual(failed["phases"], ["started", "ready", "unavailable"])

    def test_actual_correlator_requires_unique_tap_appkit_and_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            trace = [dict(eventType=k, producerTimestamp=dict(status="observed", value=k*1000000000),
                postCall=dict(start=dict(wallMillis=100, monotonicNanos=i+1))) for i,k in enumerate((5, 1, 2))]
            (root / "input-state.jsonl").write_text(json.dumps(dict(stage="after-click",kind="content",label="synthetic",clickTrace=trace)) + "\n")
            (root / "targets.jsonl").write_text(json.dumps(dict(kind="content",label="synthetic",windowPoint=[624.569, 210.5])) + "\n")
            logs, received = [], []
            for k, name in receive.KINDS.items():
                received.append(dict(kind=name, timestamp=k*1000000000))
                logs.append("buddy-appkit-receive " + json.dumps(dict(phase="receive", kind=name,
                    eventUptimeSeconds=k, x=624.569, yFromBottom=609.5, contentHeight=820)))
                logs.append("buddy-sound-dispatch " + json.dumps(dict(kind="event", label="app-root",
                    capture=True, event=name, eventPosition=[624.569, 210.5], millis=110)))
            (root / "app.log").write_text("\n".join(logs))
            receipt = dict(reason=None, stopped=True, received=received)
            observed = receive.correlate(root, receipt)
            self.assertTrue(observed["timestampRelationshipObserved"])
            self.assertFalse(observed["coverageQualified"])
            for mutation in ("duplicate", "missing", "mismatch", "unhealthy"):
                changed = json.loads(json.dumps(receipt))
                if mutation == "duplicate":
                    changed["received"] += changed["received"][:1]
                elif mutation == "missing":
                    changed["received"] = []
                elif mutation == "mismatch":
                    for row in changed["received"]:
                        row["timestamp"] += 9000000000
                else:
                    changed["reason"] = "overflow"
                self.assertFalse(receive.correlate(root, changed)["timestampRelationshipObserved"])


    def test_all_canonical_path_refusals_precede_attachment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            config = root / "config.json"
            config.write_text("{}")
            for change in ({"GITHUB_ACTIONS": "false"}, {"BUDDY_SETTINGS_FIXTURE": "0"},
                    {"BUDDY_CONFIG_PATH": str(root / "absent")}, {"RUNNER_TEMP": str(config)}):
                env = dict(GITHUB_ACTIONS="true", BUDDY_SETTINGS_FIXTURE="1",
                    RUNNER_TEMP=str(root), BUDDY_CONFIG_PATH=str(config))
                env.update(change)
                with patch.object(receive.sys, "platform", "darwin"), patch.dict(os.environ, env):
                    with self.assertRaises((RuntimeError, OSError)):
                        receive.canonical_guard()

# Actual hosted receipt projection; timestamps/receivers injected by tests only.
HOSTED_SHAPE = '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'

class ParentDefectControls(FixtureCase):
    def make_child(self, root, hang=False):
        exe = root / "protocol-child"
        exe.write_text("""#!/usr/bin/env python3
import os,time
r=int(os.environ['BUDDY_RECEIVE_READ']);w=int(os.environ['BUDDY_RECEIVE_WRITE'])
def put(x):os.write(w,(x+'\\n').encode())
def get():
 b=b''
 while not b.endswith(b'\\n'):
  z=os.read(r,1)
  if not z:raise SystemExit(2)
  b+=z
 return b.strip()
put('ready');assert get() in (b'attached',b'unavailable')
put('dispatched');assert get()==b'quiesced'
put('completed')
""" + ("time.sleep(30)\n" if hang else ""))
        exe.chmod(0o700)
        self.boundary.register_script(exe)
        return exe

    class Unavailable:
        def __init__(self, *args, **kw):
            self.finished = threading.Event()
        def start(self, deadline):
            return False
        def stop(self, deadline):
            self.finished.set()
            return dict(reason="unavailable", stopped=True, received=[], coverageQualified=False)

    def test_completed_then_hung_child_is_reaped_before_return(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            exe = self.make_child(root, True)
            created = []
            actual = receive.subprocess.Popen
            def popen(*args, **kw):
                child = actual(*args, **kw)
                created.append(child)
                return child
            try:
                with patch.object(receive, "canonical_guard"), patch.object(receive, "CONTROL_SECONDS", 1.5), patch.dict(os.environ, {"BUDDY_RECEIVER_CONTROL_EXECUTABLE":str(exe)}), patch.object(receive.subprocess, "Popen", popen):
                    with self.assertRaises(subprocess.TimeoutExpired):
                        receive.native_control(exe, root / "receipt.json", observer_factory=self.Unavailable)
                self.assertIsNotNone(created[0].returncode, "controller must reap its OWN child")
            finally:
                for child in created:
                    if child.poll() is None:
                        child.kill()
                    child.wait(timeout=2)

    def test_expired_and_late_install_admission_is_sticky(self):
        for expired in (True, False):
            with self.subTest(expired=expired):
                api = API()
                gate = threading.Event()
                original = api.install
                api.install = lambda *args: (gate.wait(1), original(*args))[1]
                with subprocess.Popen([sys.executable, "-c", "import sys;sys.stdin.buffer.read()"], stdin=subprocess.PIPE) as child:
                    with patch.object(receive, "target_guard"):
                        observer = receive.Observer(child, sys.executable, native=True, api_factory=lambda:api)
                    try:
                        try:
                            observer.start(time.monotonic() + (-1 if expired else 0.02))
                        except TimeoutError:
                            pass
                        gate.set()
                        if observer.thread is not None:
                            self.assertTrue(observer.ready.wait(1))
                        observer.callback(None, 1, 123, None)
                        self.assertFalse(observer.active)
                        self.assertIsNotNone(observer.reason)
                        self.assertEqual(observer.rows, [])
                        receipt = observer.stop(time.monotonic()+1)
                        self.assertIsNotNone(receipt["reason"])
                    finally:
                        gate.set()
                        if observer.thread is not None:
                            observer.stop(time.monotonic()+1)
                        child.stdin.close();child.stdin=None;child.wait(timeout=1)

    def test_evidence_survives_original_profile_cleanup_and_real_stage(self):
        stage_spec = importlib.util.spec_from_file_location("stage", Path(__file__).with_name("prepare-evidence.py"))
        stage = importlib.util.module_from_spec(stage_spec);stage_spec.loader.exec_module(stage)
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()
            exe=self.make_child(root)
            with tempfile.TemporaryDirectory(dir=root) as profile:
                config=Path(profile)/"config.json";config.write_text("{}")
                allocate=getattr(receive,"evidence_output",lambda _root:config.with_name("process-receive-control.json"))
                output=allocate(root)
                with patch.object(receive,"canonical_guard"),patch.dict(os.environ,{"BUDDY_RECEIVER_CONTROL_EXECUTABLE":str(exe)}):
                    result=receive.native_control(exe,output,observer_factory=self.Unavailable)
                self.assertTrue(result["functional"])
            self.assertTrue(output.is_file(), "must outlive canonical temporary profile")
            stage.stage(root/"native-evidence",root/"staged")
            copied=root/"staged"/output.relative_to(root/"native-evidence")
            self.assertEqual(copied.read_bytes(),output.read_bytes())

    def test_hosted_mixed_rows_keep_final_sound_and_reject_bad_alignment(self):
        import base64,zlib
        for encoded in (HOSTED_SHAPE, HOSTED_INTEL):
            with self.subTest(architecture="ARM" if encoded == HOSTED_SHAPE else "Intel"):
                self.check_hosted_shape(json.loads(zlib.decompress(base64.b64decode(encoded))))

    def check_hosted_shape(self, value):
        inputs,targets=value["inputs"],value["targets"]
        after=[r for r in inputs if r["stage"]=="after-click"]
        times={p["postCall"]["start"]["monotonicNanos"]:(i+1)*1000000000+j*1000000 for i,row in enumerate(after) for j,p in enumerate(row["clickTrace"])}
        logs=[];received=[]
        for row in inputs:
            for phase in row.get("clickTrace",[]):
                phase["producerTimestamp"]=dict(status="observed",value=times[phase["postCall"]["start"]["monotonicNanos"]])
        for row,target in zip(after,targets):
            point=target["windowPoint"]
            for phase in row["clickTrace"]:
                kind=receive.KINDS[phase["eventType"]];v=phase["producerTimestamp"]["value"]
                received.append(dict(kind=kind,timestamp=v))
                logs.append("buddy-appkit-receive "+json.dumps(dict(phase="receive",kind=kind,eventUptimeSeconds=v/1e9,x=point[0],yFromBottom=820-point[1],contentHeight=820)))
                logs.append("buddy-sound-dispatch "+json.dumps(dict(kind="event",label="app-root",capture=True,event=kind,eventPosition=point,millis=phase["postCall"]["start"]["wallMillis"]+1)))
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()
            (root/"targets.jsonl").write_text("\n".join(map(json.dumps,targets)))
            (root/"app.log").write_text("\n".join(logs))
            def run(rows):
                (root/"input-state.jsonl").write_text("\n".join(map(json.dumps,rows)))
                return receive.correlate(root,dict(reason=None,stopped=True,received=received))
            result=run(inputs)
            before=dict(after[0],stage="before-click")
            self.assertEqual(run([before]+inputs),result)
            self.assertEqual(len(result["joins"]),len(targets)*3)
            self.assertTrue(all(x["status"]=="observed" for x in result["joins"]))
            for rows in (inputs[:-1],inputs+[after[-1]],inputs[:-1]+[dict(after[-1],label="wrong")]):
                bad=run(rows)
                self.assertFalse(bad["timestampRelationshipObserved"])
                self.assertTrue(all(x["status"]=="unknown" for x in bad["joins"]))
            malformed=json.loads(json.dumps(inputs))
            next(row for row in malformed if row["stage"]=="after-click")["clickTrace"][0]["postCall"]={}
            self.assertFalse(run(malformed)["timestampRelationshipObserved"])

HOSTED_INTEL = 'eJztWk1z2zYQ/SsZnlUOFljsYn1L0057SduZ5pbJgZZomxOG9EiU3Tb1f+9CthXFggjHiaeZoXzQmB94WDws336QH4umu1wPq+Lk7cdiNVTndXFSVGdDvfxh3jbz98WseN90Cz3ZVVd60FandatHL+fzft3puFmxue/NsprXG5D6qu6GN39f6pHX++uz4cf1MPTdT/11V5ycVe2q1jF9txqW6/lQL36O9xcnm9mHtRpS9KerenlVLxS7mg/rqr1FKz70V7WeG/TodTXML2q9WUEUru3n1dD03cPT28kfXNiY/KfO9wmoW7ftrDhrq/PVq7aulre33syKy341vKra9s7E5cbWaz3xumnbRkcCi2OHzlhDPFMru15nbOa/VV2vl30AYiOB2HFQvDqyeRAhmAMI1iLofXxzczP7jGTYJ/lukU/heBEBnovj2wtfxzEEl2YoWE8AaCjH8RgCE0iAPY7tt3Xk9eV3TbENNk2QWA8ojCZHsT1EsSJYZzCIUvwukjwqOXHhZ03dLnaE5/fledU1/2x4+veXZvh1ffpirTR31Yf6KEaf9oDZUGoPrHUY0DjvcmI0hiAqaEgycTFi9knBV4ZEMBjHkuV4BMGjCSFMW4w4mJAmCL0JIjYbUxVBDiFYcM46SIuRctJ051+hRll9WzbnF0eBe5pnADqXlqcgHJw3KJlMYBxBRID2M4FJCRwgEiQZElBxo+CCzXE8hhAcGL8fRKYkcIAe0hFAwAsET+hzFI8hxJQWw+ME7svlKKtwumFD3KedwnGxeFHdFo9HNdtuoibMIalFMZsO6gYm4LgbRISkG0QEIc0mwExbzaxmrAc4BiOAYH3mUYsIBzhWBC3/He5zPCU1s8GnKz+9ZFWRmIzLUTyKEMQZ86jacV95XvWL+q+j5mypRkPgU1R7r0yD0b3KaE5ESD5REUGMMZpiT1tzlKG0O3vvNDSrT+d0fRwhAFo77X4VGv09QBCTY2TJUjyGgKi+jMd+1TOLETEk3ZzAeYqNcZuJHBEhuYsbBFapYjdxMVIlSQo+ATKzVQIzJfM4goqRkX3Bn5QYUUjn8RpqY4ZuQlbvRxG81nr+gBgd+1Xfs8B55yUtcKpsnr3mS5lOZkRIP3yKwBAMEE5b4LwjPMRQ4KAM53qC4whaQ1OYtsBtcqo0QWh8TFYl0xLMIJBYemRD/tiv+t/UjLy1ybcqLIEtIAJmXn0pgkt2hiOCC+C1/py2mpEWeMmvQViELGs+S5zjeAxB0xFx0+5XKUHp4joYEzvnxlJGzcYRgsZlf6tm75SFanle331qlfuo6lT/WazirJGR4iSYEtjqY8UC4pwNCtdfqh0eSw1L6MlDfOXFWgJdN4vhojghW0Y5JmtEE3sWnhUXdZTMOKrEOAoDquZqThnD4rWa1F//0TdxZ98CQGnjeB9lWeemGSgiiRirfmO9Rec2Iv304vfhImm7LAewXQe7HcPdnqHOmNLMHGJpds15Sq76TeyRtD25yPJwbh+gDMo4gGUNmhbuLdHz7CXum7pnfIO9NSz40utuGkEQGzdpx04q0ahNgbWQZsZNS+Rzu8li6aO/eAlOiGim+lAKgaaVAujEyPh67vurD1eidXdpORYwOjmQ8/5uKYpZGitgyaHWRWS3K0Eqo1fGT1WMtUj4Za4r2+EY4sxhhorITKSxRY3Q+PJsnmt3nsDHea7h5/TcL7BH0vY8yXPVjTyyV5W0eO+56EvZ/cNdz93sFW6kRdh+5rnmsK+SqpEXXaKmlQ/AVXJv/gNnaTys'

class CleanupAndEvidenceControls(FixtureCase):
    def test_symlink_and_overwrite_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()
            outside=root/"outside";outside.mkdir()
            self.boundary.symlink(root/"native-evidence",outside)
            with self.assertRaises(RuntimeError):
                receive.evidence_output(root)
            self.boundary.unlink(root/"native-evidence")
            output=receive.evidence_output(root)
            receive.write_receipt(output, {"fixed":True})
            before=output.read_bytes()
            with self.assertRaises(FileExistsError):
                receive.write_receipt(output, {"fixed":False})
            self.assertEqual(output.read_bytes(),before)
            # Replace an allocated parent with a symlink before writing.
            second=root/"second";second.mkdir()
            output=receive.evidence_output(second)
            output.parent.rmdir()
            self.boundary.symlink(output.parent,outside)
            with self.assertRaises(OSError):
                receive.write_receipt(output, {"fixed":True})
            self.assertFalse(list(outside.iterdir()))

    def test_blocked_reader_does_not_steal_owned_child_reap(self):
        with subprocess.Popen([sys.executable,"-c","import sys;sys.stdin.buffer.read()"],stdin=subprocess.PIPE) as child:
            class Blocked:
                def stop(self, deadline):
                    threading.Event().wait(max(0,deadline-time.monotonic()))
                    raise RuntimeError("fixed reader failure")
            started=time.monotonic()
            errors=receive.cleanup_owned([child],Blocked(),started+0.1)
            self.assertIsNotNone(child.returncode)
            self.assertIn("reader-not-quiescent",errors)
            self.assertLess(time.monotonic()-started,0.5)

    def test_driver_cleanup_preserves_app_exception_and_reaps(self):
        spec=importlib.util.spec_from_file_location("driver_cleanup",Path(__file__).with_name("settings-desktop.py"))
        settings=importlib.util.module_from_spec(spec);spec.loader.exec_module(settings)
        self.boundary.bind_settings(settings)
        class BadReader:
            def __init__(self,*args,**kw):self.finished=threading.Event()
            def start(self,deadline):return True
            def stop(self,deadline):raise RuntimeError("fixed cleanup failure")
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve()
            scene=root/"scene";scene.mkdir()
            driver=object.__new__(settings.Driver)
            driver.session="macos";driver.config=root/"config.json";driver.config.write_text("{}")
            driver.executable=Path(sys.executable);driver.large_size=(1200,820)
            driver.profile=root;driver.backend=unittest.mock.Mock(url="http://127.0.0.1:9")
            window=unittest.mock.Mock();window.ready.return_value=True
            original=RuntimeError("original app failure")
            with subprocess.Popen([sys.executable,"-c","import sys;sys.stdin.buffer.read()"],stdin=subprocess.PIPE) as child:
                with patch.dict(os.environ,{"GITHUB_ACTIONS":"true"}),patch.object(settings.subprocess,"Popen",return_value=child),patch.object(settings,"UnixWindow",return_value=window),patch.object(settings.time,"sleep"),patch.object(settings.process_receive,"Observer",BadReader),patch.object(driver,"interact",side_effect=original):
                    with self.assertRaises(RuntimeError) as raised:
                        driver.unix(scene,[("content","synthetic")],False,False)
                self.assertIs(raised.exception,original)
                self.assertTrue(any("reader-not-quiescent" in note for note in original.__notes__))
                self.assertIsNotNone(child.returncode)



class ParentIntegrationControls(FixtureCase):
    def test_actual_driver_admits_explicit_owned_config_without_parent_config_env(self):
        """Real Driver/Observer/guard/thread/Popen, fake window/native API only."""
        spec=importlib.util.spec_from_file_location("guard_driver",Path(__file__).with_name("settings-desktop.py"))
        settings=importlib.util.module_from_spec(spec);spec.loader.exec_module(settings)
        self.boundary.bind_settings(settings)
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve();scene=root/"scene";scene.mkdir()
            exe=root/"owned-app";exe.write_text("#!/usr/bin/env python3\nimport time\ntime.sleep(30)\n");exe.chmod(0o700)
            self.boundary.register_script(exe)
            driver=object.__new__(settings.Driver);driver.session="macos";driver.executable=exe
            driver.profile=root;driver.config=root/"config.json";driver.config.write_text("{}")
            driver.large_size=(1200,820);driver.backend=unittest.mock.Mock(url="http://127.0.0.1:9")
            class Window:
                def __init__(self,process,session):self.process,self.session=process,session
                def ready(self):return True
                def resize(self,*size):pass
            admitted=[];actual=settings.process_receive.Observer
            def observer(*args,**kw):
                value=actual(*args,**kw,api_factory=API);admitted.append(value);return value
            original=RuntimeError("fixture functional stop")
            def interact(*args):
                self.assertEqual(len(admitted),1,"actual installed caller must pass its explicit config guard")
                self.assertTrue(admitted[0].active and admitted[0].admitted)
                raise original
            env=dict(GITHUB_ACTIONS="true",BUDDY_SETTINGS_FIXTURE="1",RUNNER_TEMP=str(root))
            with patch.dict(os.environ,env),patch.object(settings.process_receive.sys,"platform","darwin"),patch.object(settings,"UnixWindow",Window),patch.object(settings.time,"sleep"),patch.object(settings.process_receive,"Observer",side_effect=observer),patch.object(driver,"interact",side_effect=interact):
                os.environ.pop("BUDDY_CONFIG_PATH",None)
                with self.assertRaises(RuntimeError) as raised:driver.unix(scene,[("content","synthetic")],False,False)
            self.assertIs(raised.exception,original)
            self.assertTrue(admitted[0].finished.is_set())
            self.assertIsNotNone(admitted[0].process.returncode)

    def test_duplicate_producer_timestamp_cannot_reuse_single_receiving_record(self):
        """Synthetic timestamps model ambiguity, never native delivery evidence."""
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory).resolve();rows=[];targets=[];logs=[];received=[]
            for i in range(2):
                trace=[dict(eventType=k,producerTimestamp=dict(status="observed",value=k*1000000000),postCall=dict(start=dict(wallMillis=100+i*100,monotonicNanos=i*10+j+1))) for j,k in enumerate((5,1,2))]
                rows.append(dict(stage="after-click",kind="content",label=f"synthetic-{i}",clickTrace=trace))
                targets.append(dict(kind="content",label=f"synthetic-{i}",windowPoint=[600,200]))
                for name in ("move","down","up"):
                    logs.append("buddy-sound-dispatch "+json.dumps(dict(kind="event",label="app-root",capture=True,event=name,eventPosition=[600,200],millis=110+i*100)))
            for k,name in receive.KINDS.items():
                received.append(dict(kind=name,timestamp=k*1000000000))
                logs.append("buddy-appkit-receive "+json.dumps(dict(phase="receive",kind=name,eventUptimeSeconds=k,x=600,yFromBottom=620,contentHeight=820)))
            (root/"input-state.jsonl").write_text("\n".join(map(json.dumps,rows)))
            (root/"targets.jsonl").write_text("\n".join(map(json.dumps,targets)))
            (root/"app.log").write_text("\n".join(logs))
            result=receive.correlate(root,dict(reason=None,stopped=True,received=received))
            self.assertFalse(result["timestampRelationshipObserved"])
            self.assertEqual(len(result["joins"]),6)
            self.assertTrue(all(row["status"]=="unknown" for row in result["joins"]))


    def test_explicit_fixture_config_preserves_all_canonical_refusals(self):
        with tempfile.TemporaryDirectory() as directory,tempfile.TemporaryDirectory() as foreign:
            root=Path(directory).resolve();config=root/"config.json";config.write_text("{}")
            outside=Path(foreign)/"config.json";outside.write_text("{}")
            linked=root/"linked.json";self.boundary.symlink(linked,outside)
            env=dict(GITHUB_ACTIONS="true",BUDDY_SETTINGS_FIXTURE="1",RUNNER_TEMP=str(root))
            with patch.dict(os.environ,env),patch.object(receive.sys,"platform","darwin"):
                os.environ.pop("BUDDY_CONFIG_PATH",None)
                receive.canonical_guard(config)
                for rejected in (outside,linked,root/"absent"):
                    with self.assertRaises((RuntimeError,OSError)):receive.canonical_guard(rejected)
                with patch.dict(os.environ,{"BUDDY_SETTINGS_FIXTURE":"0"}):
                    with self.assertRaises(RuntimeError):receive.canonical_guard(config)
