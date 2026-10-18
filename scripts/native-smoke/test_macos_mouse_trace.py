"""Lifecycle regressions; no native APIs, desktops, credentials or UI are used."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("mouse_trace", Path(__file__).with_name("macos-mouse-trace.py"))
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class MouseTraceLifecycle(unittest.TestCase):
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
