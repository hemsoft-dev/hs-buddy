"""Real recorded-accounts recipe/click/camera consumers; fake OS only."""
import ast
import copy
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch
import desktop

def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value
settings, inputs = load("settings-desktop"), load("macos-input")

class RecordedInput(unittest.TestCase):
    def test_exact_accounts_recipe_requests_recording_with_false_control(self):
        tree = ast.parse(Path(__file__).with_name("settings-desktop.py").read_text())
        body = next(node.body for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "qualify")
        start = next(i for i, node in enumerate(body) if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "account_actions" for target in node.targets))
        stop = next(i for i in range(start, len(body)) if isinstance(body[i], ast.Assign) and isinstance(body[i].value, ast.Call) and isinstance(body[i].value.func, ast.Attribute) and body[i].value.func.attr == "drive")
        namespace = {}
        exec(compile(ast.Module(body=body[start:stop], type_ignores=[]), "actual-account-actions", "exec"), namespace)
        actions = namespace["account_actions"]
        self.assertEqual(sum(action[0] == "rightfield" for action in actions), 2)
        for recorded in (True, False):
            call = copy.deepcopy(body[stop].value)
            if not recorded:
                next(keyword for keyword in call.keywords if keyword.arg == "record").value = ast.Constant(False)
            with tempfile.TemporaryDirectory() as raw, patch.object(settings, "encode_recording") as encode:
                driver = object.__new__(settings.Driver)
                driver.profile, driver.session, driver.unix = Path(raw), "macos", Mock()
                eval(compile(ast.fix_missing_locations(ast.Expression(call)), "actual-account-drive", "eval"), dict(namespace, driver=driver))
                self.assertIs(driver.unix.call_args.args[3], recorded)
                self.assertEqual(driver.unix.call_args.args[1], actions)
                self.assertEqual(encode.call_count, int(recorded))

    def test_recording_camera_cannot_interleave_real_rightfield_single_click(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            moved, attempted, first, second = (threading.Event() for _ in range(4))
            trace = []
            class Window(desktop.UnixWindow):
                def screenshot(self, path):
                    if moved.is_set(): attempted.set()
                    super().screenshot(path)
            process = Mock(pid=321)
            process.poll.return_value = None
            window = Window(process, "macos")
            driver = object.__new__(settings.Driver)
            driver.session = "macos"
            driver.input_receipt, driver.capture_click_boundary = Mock(), Mock()
            def png(path): path.write_bytes(b"0" * 16 + struct.pack(">II", 1200, 820))
            def locate(_window, image, *_args, **_kwargs):
                png(image)
                image.with_suffix(".window.json").write_text(json.dumps(dict(kCGWindowBounds=dict(X=80, Y=80, Width=1200, Height=820), capturePixelsPerWindowPoint=1)))
                return dict(left=73, top=300, width=90, height=14)
            driver.locate = Mock(side_effect=locate)
            helper = object.__new__(inputs.OwnedMacInput)
            helper.process, helper.graphics, helper.core = process, Mock(), Mock()
            helper.release_modifiers, helper.replace_text = Mock(), Mock()
            helper.graphics.CGPreflightPostEventAccess.return_value = True
            helper.graphics.CGEventSourceButtonState.return_value = False
            helper.graphics.CGEventCreateMouseEvent.side_effect = lambda _source, kind, _point, _button: kind
            def post(_tap, event):
                trace.append(event)
                if event == 5:
                    moved.set()
                    if not attempted.wait(3): raise RuntimeError("Camera did not attempt the second frame")
                    second.wait(0.25)  # Test synchronization only, no product delay.
            helper.graphics.CGEventPost.side_effect = post
            def command(*args, **_kwargs):
                if args[0] == "screencapture":
                    trace.append("capture")
                    png(Path(args[-1]))
                    (second if moved.is_set() else first).set()
                else:
                    return json.dumps(dict(kCGWindowNumber=9, kCGWindowBounds=dict(Width=1200, Height=820)))
            with patch.object(settings, "OwnedMacInput", return_value=helper), \
                    patch.object(settings, "run", side_effect=command), \
                    patch.object(desktop, "run", side_effect=command), \
                    patch.object(inputs.time, "sleep"), patch.dict("os.environ", RUNNER_TEMP=raw):
                with desktop.recording(window, root / "frames"):
                    self.assertTrue(first.wait(3))
                    driver.interact(window, root, "rightfield", ["Organization|GitHub username", "fixture-native-team"])
                    self.assertTrue(second.wait(3))
            self.assertEqual([value for value in trace if isinstance(value, int)], [5, 1, 2])
            self.assertNotIn("capture", trace[trace.index(5):trace.index(2) + 1], "Recording command interleaved with one actual click")
            self.assertFalse(window._recording)
            helper.replace_text.assert_called_once_with("fixture-native-team")
            self.assertGreaterEqual(len(list((root / "frames").glob("frame-*.png"))), 2)

    def test_failure_releases_reentrant_guard_and_unrecorded_platforms_do_not_lock(self):
        window = desktop.UnixWindow(Mock(), "macos")
        window._recording = True
        with self.assertRaisesRegex(RuntimeError, "controlled"):
            with desktop.mac_input_guard(window):
                with desktop.mac_input_guard(window):
                    raise RuntimeError("controlled")
        acquired = []
        def other():
            with desktop.mac_input_guard(window): acquired.append(True)
        worker = threading.Thread(target=other, daemon=True)
        worker.start(); worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertEqual(acquired, [True])
        for session, recorded in (("macos", False), ("x11", True), ("wayland", True)):
            value = desktop.UnixWindow(Mock(), session)
            value._recording, value._recording_input_lock = recorded, Mock()
            with desktop.mac_input_guard(value): pass

    def test_actual_recording_failure_restores_scope_and_original_exception(self):
        with tempfile.TemporaryDirectory() as raw:
            window = desktop.UnixWindow(Mock(), "macos")
            ready = threading.Event()
            frames = []
            def capture(path):
                path.write_bytes(b"synthetic retained frame")
                frames.append(path)
                if len(frames) >= 2: ready.set()
            failure = RuntimeError("controlled recorded input failure")
            with patch.object(window, "screenshot", side_effect=capture):
                with self.assertRaises(RuntimeError) as raised:
                    with desktop.recording(window, Path(raw) / "frames"):
                        self.assertTrue(ready.wait(3))
                        self.assertTrue(window._recording)
                        with desktop.mac_input_guard(window):
                            raise failure
            self.assertIs(raised.exception, failure)
            self.assertFalse(window._recording)
            self.assertGreaterEqual(len(frames), 2)
