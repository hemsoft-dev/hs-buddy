"""Portable real-consumer controls; OS/native values are explicitly modeled."""
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import patch

import desktop

HERE = Path(__file__).parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


placement = load("settings-placement")
settings = load("settings-desktop")


def display():
    return dict(space="cg-top-left-points", main=True, single=True, scale=1,
                bounds=dict(x=0, y=0, width=1600, height=1200),
                usable=dict(x=0, y=30, width=1600, height=1102))


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.env = patch.dict(os.environ, RUNNER_TEMP=str(self.root), GITHUB_ACTIONS="true",
                              BUDDY_SETTINGS_FIXTURE="1", BUDDY_SETTINGS_DISPLAY_RECEIPT=str(self.root / "display.json"))
        self.env.start()
        self.addCleanup(self.env.stop)
        self.mac = patch.object(placement.sys, "platform", "darwin")
        self.mac.start()
        self.addCleanup(self.mac.stop)
        self.machine = patch.object(placement.platform, "machine", return_value="arm64")
        self.machine.start()
        self.addCleanup(self.machine.stop)
        (self.root / "display.json").write_text(json.dumps(dict(comparisonDisplay=display())))
        self.config = self.root / "profile" / "config.json"
        self.config.parent.mkdir()
        self.config.write_text("{}")
        (self.config.parent / ".buddy-settings-fixture").write_text("buddy settings fixture v1")
        self.executable = self.root / "Buddy Native.app/Contents/MacOS/buddy-native"
        self.executable.parent.mkdir(parents=True)
        self.executable.write_bytes(b"synthetic package binary, never executed")
        child = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(30)"], stdin=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        self.addCleanup(self.reap, child)
        # Actual owned OS Popen, explicitly FAKE app executable metadata.
        child.args = [str(self.executable)]
        self.child = child
        self.window = desktop.UnixWindow(child, "macos")
        self.arm = placement.Arm((80, 80), self.executable)
        self.arm.attach(self.window, self.config, scene="preferences")

    @staticmethod
    def reap(child):
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)

    def metadata(self, arm=None):
        arm = arm or self.arm
        return dict(comparisonDisplay=display(), kCGWindowBounds=dict(X=arm.origin[0], Y=80, Width=1200, Height=820),
                    kCGWindowOwnerPID=self.child.pid, kCGWindowNumber=1,
                    capturePixelSize=[1200, 820], capturePixelsPerWindowPoint=1)


class GeometryControls(Fixture):
    def test_optional_geometry_uses_equal_operations_and_existing_wait(self):
        calls = []
        def run(*args, **kwargs):
            calls.append(args)
            return "1200, 820, 80, 80" if len(calls) == 2 else ""
        with patch.object(desktop, "run", side_effect=run), patch.object(desktop.time, "sleep") as sleep:
            self.window.resize(1200, 820, initial_origin=(80, 80))
        self.assertIn("set size of window 1 to {1200, 820}\nset position of window 1 to {80, 80}", calls[0][-1])
        self.assertEqual(len(calls), 2)
        sleep.assert_called_once_with(1)
        self.assertTrue(self.arm.assigned)

    def test_default_resize_has_original_two_calls_and_no_position(self):
        with patch.object(desktop, "run", side_effect=["", "1200, 820"]) as run, patch.object(desktop.time, "sleep") as sleep:
            self.window.resize(1200, 820)
        self.assertEqual(run.call_count, 2)
        self.assertIn("get size of window 1", run.call_args.args[-1])
        self.assertNotIn("position", run.call_args_list[0].args[-1])
        sleep.assert_called_once_with(1)

    def test_wrong_assignment_fails_before_input(self):
        with patch.object(desktop, "run", side_effect=["", "1200,820,81,80"]), patch.object(desktop.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "not applied"):
                self.window.resize(1200, 820, initial_origin=(80, 80))
        self.assertFalse(self.arm.assigned)

    def test_guard_and_invalid_origins_refuse_before_os_call(self):
        with patch.object(desktop, "run") as run:
            for origin in [(241,80), (float("nan"),80), (80,81)]:
                with self.assertRaises(RuntimeError):
                    self.window.resize(1200, 820, initial_origin=origin)
            with patch.object(placement.platform, "machine", return_value="x86_64"):
                with self.assertRaises(RuntimeError):
                    self.window.resize(1200, 820, initial_origin=(80,80))
            self.window.session = "x11"
            with self.assertRaises(RuntimeError):
                self.window.resize(1200, 820, initial_origin=(80,80))
        run.assert_not_called()

    def test_non_preferences_scene_refused_before_geometry(self):
        with self.assertRaises(RuntimeError):
            placement.Arm((80,80),self.executable).attach(self.window,self.config,scene="accounts")

    def test_missing_outside_ambiguous_display_refuses_before_position(self):
        for mutation in [
                lambda d:d["usable"].update(x=100),
                lambda d:d["usable"].update(width=1400),
                lambda d:d.update(single=False),
                lambda d:d.update(scale=float("nan")),
                lambda d:d.pop("usable")]:
            data = display()
            mutation(data)
            (self.root / "display.json").write_text(json.dumps(dict(comparisonDisplay=data)))
            with patch.object(desktop, "run") as run, self.assertRaises(RuntimeError):
                self.window.resize(1200,820,initial_origin=(80,80))
            run.assert_not_called()

    def test_existing_capture_checks_geometry_before_locator_input(self):
        self.arm.assigned_geometry([1200,820,80,80])
        self.arm.capture(self.metadata())
        for field, value in [("capturePixelsPerWindowPoint",2), ("capturePixelSize",[2400,1640]),
                             ("kCGWindowOwnerPID",-1), ("kCGWindowBounds",dict(X=81,Y=80,Width=1200,Height=820))]:
            data=self.metadata()
            data[field]=value
            with self.assertRaises(RuntimeError):
                self.arm.capture(data)
        self.reap(self.child)
        with self.assertRaises(RuntimeError):
            self.arm.capture(self.metadata())

    def test_actual_capture_consumer_checks_tuple_pixels_and_gated_helper_environment(self):
        self.arm.assigned_geometry([1200,820,80,80])
        path=self.root/"actual-capture.png"
        def run(*args, **kwargs):
            if str(args[0]).endswith("buddy-window-info"):
                self.assertEqual(kwargs["env"]["BUDDY_SETTINGS_COMPARISON"],placement.MODE)
                return json.dumps(self.metadata())
            self.assertEqual(args[0],"screencapture")
            path.write_bytes(b"\x89PNG\r\n\x1a\n"+b"\x00"*8+(1200).to_bytes(4,"big")+(820).to_bytes(4,"big"))
        with patch.object(desktop,"run",side_effect=run) as calls:
            self.window.screenshot(path)
        self.assertEqual(calls.call_count,2)
        self.assertEqual(self.arm.window_id,1)
        self.assertEqual(json.loads(path.with_suffix(".window.json").read_text())["capturePixelSize"],[1200,820])

    def test_package_provenance_actual_files_and_checksum_refusal(self):
        package=self.root/"package";package.mkdir()
        artifact=package/"fixture.dmg";artifact.write_bytes(b"synthetic package")
        manifest=dict(sourceSha="a"*40,sourceDirty=False,profile="release",version="1",
                      binarySha256="c"*64,
                      package=dict(file=artifact.name,sha256=placement.digest(artifact)))
        info=self.executable.parent.parent/"Info.plist"
        info.write_bytes(plistlib.dumps(dict(CFBundleIdentifier="com.hemsoft.buddy.native",CFBundleShortVersionString="1")))
        (package/"build.json").write_text(json.dumps(manifest))
        with patch.object(placement.subprocess,"check_output",return_value="a"*40):
            actual=placement.provenance(self.executable,package,self.root)
            self.assertEqual(actual["binarySha256"],placement.digest(self.executable))
            self.assertEqual(actual["stagedBinarySha256"],manifest["binarySha256"])
            self.assertNotEqual(actual["stagedBinarySha256"],actual["binarySha256"])
            artifact.write_bytes(b"changed")
            with self.assertRaises(RuntimeError):
                placement.provenance(self.executable,package,self.root)
        self.assertFalse((self.root/"pair").exists())

    def test_dead_owned_target_refused_without_foreign_pid_attachment(self):
        self.reap(self.child)
        with self.assertRaises(RuntimeError):
            placement.Arm((80,80),self.executable).attach(self.window,self.config,scene="preferences")

    def test_actual_interact_translates_target_without_new_clicks(self):
        outputs=[]
        for origin in placement.ORIGINS:
            arm=placement.Arm(origin,self.executable)
            arm.attach(self.window,self.config,scene="preferences")
            arm.assigned_geometry([1200,820,*origin])
            arm.capture(self.metadata(arm))
            directory=self.root/str(origin[0]);directory.mkdir()
            image=directory/"locator.png"
            image.write_bytes(b"\x89PNG\r\n\x1a\n"+b"\x00"*8+(1200).to_bytes(4,"big")+(820).to_bytes(4,"big"))
            image.with_suffix(".window.json").write_text(json.dumps(self.metadata(arm)))
            calls=[]
            class Input:
                def __init__(self,process):pass
                def click(self,*point):calls.append(point)
            driver=settings.Driver.__new__(settings.Driver);driver.session="macos"
            driver.locate=lambda *a,**k:dict(left=530,top=201,width=189,height=19)
            driver.input_receipt=lambda *a:None
            driver.capture_click_boundary=lambda *a:None
            with patch.object(settings,"OwnedMacInput",Input),patch.object(settings,"run"),patch.object(settings.time,"sleep"):
                driver.interact(self.window,directory,"content",["Review completion sound: off"])
            self.assertEqual(len(calls),1)
            self.assertEqual(arm.targets[0]["point"],[624.5,210.5])
            outputs.append(calls[0])
        self.assertEqual(outputs,[(704.5,290.5),(864.5,290.5)])

    def test_target_refuses_window_and_control_escape_or_missing_capture(self):
        bounds=dict(left=530,top=201,width=189,height=19)
        with self.assertRaises(RuntimeError):
            self.arm.target("content","sound",[624.5,210.5],bounds)
        self.arm.assigned_geometry([1200,820,80,80]);self.arm.capture(self.metadata())
        for point in [[1200,210.5],[624.5,float("nan")],[500,210.5]]:
            with self.assertRaises(RuntimeError):
                self.arm.target("content","sound",point,bounds)


class PairControls(Fixture):
    def backend(self, checkout):
        owner=self
        class Backend:
            def __init__(self):
                self.child=None;self.owns_local_state=False
                self.generated=owner.root/"absent-env"
                self.log=None
            def __enter__(self):
                self.child=subprocess.Popen([sys.executable,"-c","pass"],stdin=subprocess.DEVNULL, creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                self.child.wait(timeout=5)
                return self
            def __exit__(self,*args):return False
        return Backend()

    def run_pair(self, action):
        self.calls=[]
        identity=dict(sourceSha="a"*40,version="1",packageSha256="b"*64,binarySha256="c"*64)
        def qualify(executable, output, session, checkout, backend, *, placement):
            self.calls.append(output.name)
            output.mkdir()
            placement.assigned=True
            placement.display=display()
            placement.targets=[dict(kind="content",label="Review completion sound: off",point=[624.5,210.5],bounds={})]
            event=threading.Event();event.set()
            observer=SimpleNamespace(finished=event,reason=None,receipt=lambda:dict(stopped=True))
            placement.cleaned([backend.child],observer,[])
            return action(placement, output, backend)
        self.output=self.root/"pair"
        with patch.object(placement,"provenance",return_value=identity):
            return placement.run_pair(self.executable,self.output,self.root,self.root,qualify,self.backend)

    def receipt(self):
        return json.loads((self.output/"placement-comparison.json").read_text())

    def test_original_a_exception_identity_retained_after_safe_b_and_durable_partial(self):
        failure=RuntimeError(placement.SOUND_FAILURE)
        def action(arm,output,backend):
            partial=self.receipt()
            self.assertEqual(partial["arms"][-1]["status"],"started")
            if output.name=="A":raise failure
            (output/"qualification.json").write_text('{"result":"passed"}')
        with self.assertRaises(RuntimeError) as caught:self.run_pair(action)
        self.assertIs(caught.exception,failure)
        self.assertEqual(self.calls,["A","B"])
        receipt=self.receipt()
        self.assertEqual(receipt["result"],"placement-dependent-in-this-pair")
        self.assertFalse(receipt["coverageQualified"])
        self.assertFalse((self.output/"A/qualification.json").exists())

    def test_b_later_failure_is_not_pass_and_a_failure_keeps_identity(self):
        a=RuntimeError(placement.SOUND_FAILURE)
        def action(arm,output,backend):
            raise a if output.name=="A" else RuntimeError("later original assertion")
        with self.assertRaises(RuntimeError) as caught:self.run_pair(action)
        self.assertIs(caught.exception,a)
        self.assertEqual(self.receipt()["result"],"inconclusive")
        self.assertEqual([r["status"] for r in self.receipt()["arms"]],["failed","failed"])

    def test_actual_subprocess_failure_exit_and_partial_receipt_staging(self):
        script = (
            "import sys;sys.path.insert(0,sys.argv[1]);"
            "from test_settings_placement import PairControls,placement;"
            "c=PairControls();c.setUp();"
            "\ntry:\n c.run_pair(lambda *a:(_ for _ in ()).throw(RuntimeError(placement.SOUND_FAILURE)))"
            "\nfinally:\n c.doCleanups()"
        )
        result=subprocess.run([sys.executable,"-c",script,str(HERE)],capture_output=True,timeout=15,
                              creationflags=subprocess.CREATE_NO_WINDOW if os.name=="nt" else 0)
        self.assertNotEqual(result.returncode,0)
        self.assertIn(placement.SOUND_FAILURE,result.stderr.decode())
        failure=RuntimeError(placement.SOUND_FAILURE)
        with self.assertRaises(RuntimeError):
            self.run_pair(lambda *a:(_ for _ in ()).throw(failure))
        staging=load("prepare-evidence")
        staging.stage(self.output,self.root/"upload")
        self.assertEqual(json.loads((self.root/"upload/placement-comparison.json").read_text()),self.receipt())

    def test_a_green_is_inconclusive_and_b_failure_propagates(self):
        failure=RuntimeError("B original assertion")
        def action(arm,output,backend):
            if output.name=="B":raise failure
        with self.assertRaises(RuntimeError) as caught:self.run_pair(action)
        self.assertIs(caught.exception,failure)
        self.assertEqual(self.receipt()["result"],"inconclusive")

    def test_both_sound_red_is_negative_for_shift_not_passing(self):
        with self.assertRaises(RuntimeError):
            self.run_pair(lambda *a:(_ for _ in ()).throw(RuntimeError(placement.SOUND_FAILURE)))
        self.assertEqual(self.receipt()["result"],"not-supported-for-this-shift")

    def test_unsafe_cleanup_and_admission_each_stop_b(self):
        for failure in ["cleanup","admission","backend","observer"]:
            if hasattr(self,"output"):shutil.rmtree(self.output)
            def action(arm,output,backend):
                if failure=="cleanup":arm.cleanup.append(False)
                if failure=="backend":backend.owns_local_state=True
                if failure=="observer":arm.healthy=False
                if failure=="admission":raise placement.AdmissionError("wrong geometry")
                raise RuntimeError(placement.SOUND_FAILURE)
            with self.assertRaises(RuntimeError):self.run_pair(action)
            self.assertEqual(self.calls,["A"])

    def test_layout_scale_or_health_drift_invalidates_comparison(self):
        for failure in ["layout","scale","health"]:
            if hasattr(self,"output"):shutil.rmtree(self.output)
            def action(arm,output,backend):
                if output.name=="A":raise RuntimeError(placement.SOUND_FAILURE)
                if failure=="layout":arm.targets[0]["point"][0]+=1
                if failure=="scale":arm.display["scale"]=2
                if failure=="health":arm.healthy=False
            with self.assertRaises(RuntimeError):self.run_pair(action)
            self.assertEqual(self.receipt()["result"],"inconclusive")

    def test_interruption_retains_partial_receipt_and_never_starts_b(self):
        failure=KeyboardInterrupt()
        with self.assertRaises(KeyboardInterrupt) as caught:
            self.run_pair(lambda *a:(_ for _ in ()).throw(failure))
        self.assertIs(caught.exception,failure)
        self.assertEqual(self.calls,["A"])
        self.assertEqual(self.receipt()["arms"][0]["status"],"failed")

    def test_shared_deadline_does_not_reset_before_b(self):
        failure=RuntimeError(placement.SOUND_FAILURE)
        with patch.object(placement.time,"monotonic",side_effect=[0,0,1201]):
            with self.assertRaises(RuntimeError) as caught:
                self.run_pair(lambda *a:(_ for _ in ()).throw(failure))
        self.assertIs(caught.exception,failure)
        self.assertEqual(self.calls,["A"])

    def test_bad_display_refuses_both_qualifiers_before_any_input(self):
        data=display();data["usable"]["width"]=1400
        (self.root/"display.json").write_text(json.dumps(dict(comparisonDisplay=data)))
        with self.assertRaises(RuntimeError):
            self.run_pair(lambda *a:None)
        self.assertEqual(self.calls,[])
        self.assertFalse(self.output.exists())

    def test_actual_main_routes_pair_once_and_default_qualifier_once(self):
        output=self.root/"main-output"
        base=["settings-desktop.py","--executable",str(self.executable),"--output",str(output),"--session","macos"]
        with patch.object(settings.sys,"argv",base),patch.object(settings,"SettingsBackend") as backend,patch.object(settings,"qualify") as qualify:
            settings.main()
            qualify.assert_called_once()
            self.assertNotIn("placement",qualify.call_args.kwargs)
        with patch.object(settings.sys,"argv",base+["--compare-placement","--package-directory",str(self.root)]),patch.object(settings.placement_compare,"run_pair") as pair,patch.object(settings,"qualify") as qualify:
            settings.main()
            pair.assert_called_once()
            qualify.assert_not_called()

    def test_a_primary_survives_b_admission_failure(self):
        primary=RuntimeError(placement.SOUND_FAILURE)
        with patch.object(placement,"display_receipt",side_effect=[display(),display(),RuntimeError("synthetic unavailable")]):
            with self.assertRaises(RuntimeError) as caught:
                self.run_pair(lambda *a:(_ for _ in ()).throw(primary))
        self.assertIs(caught.exception,primary)
        self.assertEqual(self.calls,["A"])

    def test_a_primary_survives_b_start_receipt_failure(self):
        primary=RuntimeError(placement.SOUND_FAILURE)
        writer=placement.write_receipt
        def write(output,value):
            if len(value["arms"])==2 and value["arms"][-1]["status"]=="started":
                raise OSError("synthetic storage failure")
            return writer(output,value)
        def action(arm,output,backend):
            if output.name=="A":raise primary
        with patch.object(placement,"write_receipt",side_effect=write):
            with self.assertRaises(RuntimeError) as caught:self.run_pair(action)
        self.assertIs(caught.exception,primary)
        self.assertEqual(self.calls,["A"])
        self.assertEqual(self.receipt()["arms"][0]["status"],"failed")

    def test_a_primary_survives_final_receipt_failure(self):
        primary=RuntimeError(placement.SOUND_FAILURE)
        writer=placement.write_receipt
        completed=0
        def write(output,value):
            nonlocal completed
            if len(value["arms"])==2 and value["arms"][-1]["status"]=="passed":
                completed+=1
                if completed==2:raise OSError("synthetic storage failure")
            return writer(output,value)
        def action(arm,output,backend):
            if output.name=="A":raise primary
        with patch.object(placement,"write_receipt",side_effect=write):
            with self.assertRaises(RuntimeError) as caught:self.run_pair(action)
        self.assertIs(caught.exception,primary)
        self.assertEqual(self.calls,["A","B"])
        self.assertEqual(self.receipt()["arms"][1]["status"],"passed")

    def test_original_exception_survives_receipt_failure(self):
        primary=RuntimeError("original")
        with patch.object(placement,"write_receipt",side_effect=OSError("synthetic write failure")):
            with self.assertRaises(RuntimeError) as caught:placement.persist(self.root,{},primary)
        self.assertIs(caught.exception,primary)

    def test_guard_refuses_comparison_before_provenance_or_process(self):
        with patch.object(placement.platform,"machine",return_value="x86_64"),patch.object(placement,"provenance") as provenance:
            with self.assertRaises(RuntimeError):
                placement.run_pair(self.executable,self.root/"new",self.root,self.root,None,None)
        provenance.assert_not_called()

    def test_fresh_output_and_path_escapes_refused(self):
        with self.assertRaises(RuntimeError):
            placement.owned_path(self.root.parent/"outside",exists=False)
        target=self.root/"receipt"
        if os.name != "nt":
            target.symlink_to(self.config)
            with self.assertRaises(RuntimeError):
                placement.owned_path(target)
        else:
            # Model symlink resolution without requesting Windows symlink privilege.
            target.write_text("modeled link boundary")
            real_resolve = Path.resolve
            def resolve(path, *args, **kwargs):
                return self.config if path == target else real_resolve(path, *args, **kwargs)
            with patch.object(Path, "resolve", resolve), self.assertRaises(RuntimeError):
                placement.owned_path(target)
        self.output=self.root/"pair";self.output.mkdir()
        with patch.object(placement,"provenance",return_value={}):
            with self.assertRaises(RuntimeError):placement.run_pair(self.executable,self.output,self.root,self.root,None,None)


if __name__ == "__main__":
    unittest.main()
