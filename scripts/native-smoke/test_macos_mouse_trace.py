"""Lifecycle regressions; no native APIs, desktops, credentials or UI are used."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("mouse_trace", Path(__file__).with_name("macos-mouse-trace.py"))
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class MouseTraceLifecycle(unittest.TestCase):
    def dialog_rows(self):
        window = dict(available=True, windowNumber=27, kCGWindowOwnerName="UserNotificationCenter", kCGWindowOwnerPID=99)
        return [dict(ownedPid=123, referencedWindows=[window, dict(window, windowNumber=28, kCGWindowOwnerName="Another app")]),
            dict(ownedPid=123, referencedWindows=[window])]

    def test_capture_is_one_exact_intercepted_dialog_and_never_the_whole_display(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            target = directory / "foreign-system-dialog-27.png"
            def capture(command, **kwargs):
                self.assertEqual(command, ["screencapture", "-x", "-o", "-l", "27", str(target)])
                self.assertEqual(kwargs, dict(check=True, capture_output=True, timeout=10))
                target.write_bytes(b"Synthetic window capture fixture")
            with patch.object(trace.sys, "platform", "darwin"), patch.dict(os.environ, GITHUB_ACTIONS="true", BUDDY_SETTINGS_FIXTURE="1"), patch.object(trace.subprocess, "run", side_effect=capture) as run:
                trace._capture_dialogs(self.dialog_rows(), directory)
            run.assert_called_once()
            self.assertEqual(target.read_bytes(), b"Synthetic window capture fixture")

    def test_dialog_capture_refuses_non_ci_before_any_native_call(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, GITHUB_ACTIONS="false"), patch.object(trace.subprocess, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "disposable CI Mac"):
                trace._capture_dialogs(self.dialog_rows(), Path(temp))
            run.assert_not_called()

    def test_dialog_capture_failure_preserves_the_exact_primary_application_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            primary = RuntimeError("Actual failed account assertion")
            with patch.object(trace.subprocess, "Popen", side_effect=lambda *a, **k: self.fixture(directory)), patch.object(trace, "_capture_dialogs", side_effect=OSError("Synthetic screenshot failure")) as capture:
                with self.assertRaises(RuntimeError) as observed:
                    with trace._owned_monitor("monitor", 123, directory):
                        raise primary
            self.assertIs(observed.exception, primary)
            self.assertIn("primary failure retained", primary.__notes__[0])
            capture.assert_called_once()

    def fixture(self, directory, *, code=-15, disabled=False):
        child = Mock()
        child.poll.return_value = None
        child.wait.return_value = code
        (directory / "native-mouse-ready.json").write_text(json.dumps(
            dict(result="listening", ownedPid=123, listenOnly=True, mouseOnly=True)))
        (directory / "native-mouse-events.jsonl").write_text(json.dumps(
            dict(ownedPid=123, type=1, tapDisabled=disabled)) + "\n")
        return child

    def test_success_reaps_exact_owned_monitor(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            child = None
            def launch(*args, **kwargs):
                nonlocal child
                self.assertEqual(args[0], ["monitor", "123", str(directory)])
                child = self.fixture(directory)
                return child
            with patch.object(trace.subprocess, "Popen", side_effect=launch) as start:
                with trace._owned_monitor("monitor", 123, directory):
                    pass
            self.assertEqual(start.call_count, 1)
            child.terminate.assert_called_once()
            child.wait.assert_called_once_with(timeout=10)

    def test_application_failure_identity_survives_monitor_shutdown_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            child = None
            def launch(*args, **kwargs):
                nonlocal child
                child = self.fixture(directory, code=2)
                return child
            primary = RuntimeError("Actual application assertion")
            with patch.object(trace.subprocess, "Popen", side_effect=launch) as start:
                with self.assertRaises(RuntimeError) as observed:
                    with trace._owned_monitor("monitor", 123, directory):
                        raise primary
            self.assertIs(observed.exception, primary)
            self.assertIn("primary failure retained", primary.__notes__[0])
            self.assertEqual(start.call_count, 1)
            child.terminate.assert_called_once()
            child.wait.assert_called_once_with(timeout=10)

    def test_required_monitor_failure_fails_successful_exercise(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            with patch.object(trace.subprocess, "Popen", side_effect=lambda *a, **k: self.fixture(directory, disabled=True)):
                with self.assertRaisesRegex(RuntimeError, "valid owned-process events"):
                    with trace._owned_monitor("monitor", 123, directory):
                        pass

    def test_prior_evidence_prevents_startup(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            original = b'{"result":"listening","ownedPid":123}'
            (directory / "native-mouse-ready.json").write_bytes(original)
            with patch.object(trace.subprocess, "Popen") as start:
                with self.assertRaisesRegex(RuntimeError, "earlier evidence"):
                    with trace._owned_monitor("monitor", 123, directory):
                        pass
            start.assert_not_called()
            self.assertEqual((directory / "native-mouse-ready.json").read_bytes(), original)

    def test_startup_exit_cannot_use_absent_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            child = Mock()
            child.poll.return_value = 1
            child.wait.return_value = 1
            with patch.object(trace.subprocess, "Popen", return_value=child):
                with self.assertRaisesRegex(RuntimeError, "before listening"):
                    with trace._owned_monitor("monitor", 123, directory):
                        self.fail("Failed startup must not execute the exercise")
            child.wait.assert_called_once_with(timeout=10)


if __name__ == "__main__":
    unittest.main()
