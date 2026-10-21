import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("automation_desktop", Path(__file__).with_name("automation-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def shell_argv(command):
    """The argv the native exec worker builds for the job's shell."""
    if os.name == "nt":
        return ["pwsh.exe", "-NoProfile", "-NonInteractive", "-Command",
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; " + command]
    return ["bash", "-c", command]


class AutomationDesktopTests(unittest.TestCase):
    def test_non_runner_is_refused_before_argument_file_backend_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser, \
                patch.object(desktop.settings, "SettingsBackend") as backend:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()
            backend.assert_not_called()

    def test_echo_job_runs_in_its_shell_with_unicode_directory_and_environment(self):
        with tempfile.TemporaryDirectory() as root:
            workspace = Path(root) / "workspace with spaces 資料"
            workspace.mkdir()
            proof = Path(root) / "proof.json"
            arguments = desktop.job_arguments("Echo job", desktop.echo_command(proof, "nonce-1"), workspace)
            environment = dict(os.environ, **arguments["config"]["env"])
            flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
            result = subprocess.run(shell_argv(arguments["config"]["command"]), cwd=workspace, env=environment,
                capture_output=True, timeout=60, creationflags=flags)
            self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", "replace"))
            output = result.stdout.decode("utf-8")
            self.assertIn("NATIVEAUTOMATIONREADY", output)
            self.assertIn(desktop.UNICODE, output)
            self.assertNotIn("NATIVEAUTOMATIONREADY", arguments["config"]["command"])
            desktop.verify_proof(proof, "nonce-1", workspace)
            with self.assertRaisesRegex(RuntimeError, "did not run once"):
                desktop.verify_proof(proof, "another-run", workspace)
            with self.assertRaisesRegex(RuntimeError, "did not run once"):
                desktop.verify_proof(proof, "nonce-1", Path(root))

    def test_runs_must_be_single_and_in_the_expected_state(self):
        backend = Mock()
        backend.query.return_value = [dict(jobId="job", status="completed",
            output=dict(stdout="NATIVEAUTOMATIONREADY\n資料界é\n", stderr="", exitCode=0)),
            dict(jobId="other", status="failed")]
        desktop.verify_output(desktop.single_run(backend, "job", "completed"))
        with self.assertRaisesRegex(RuntimeError, "cancelled"):
            desktop.single_run(backend, "job", "cancelled")
        backend.query.return_value.append(dict(jobId="job", status="completed"))
        with self.assertRaisesRegex(RuntimeError, "exactly one run"):
            desktop.single_run(backend, "job", "completed")
        for output in (None, "NATIVEAUTOMATIONREADY 資料界é", {"stdout": "NATIVEAUTOMATIONREADY"},
                {"stdout": "NATIVEAUTOMATIONREADY 資料界é", "exitCode": 1}):
            with self.assertRaisesRegex(RuntimeError, "Unicode output"):
                desktop.verify_output(dict(output=output))

    def test_a_surviving_cancelled_process_fails_and_the_sentinel_is_checked(self):
        with tempfile.TemporaryDirectory() as root:
            receipts = Path(root) / "children.jsonl"
            receipts.write_text(json.dumps(dict(run="old", pids=[5, 6])) + "\n"
                + json.dumps(dict(run="now", pids=[7, 8])) + "\n", encoding="utf-8")
            sentinel = Mock()
            sentinel.poll.return_value = None
            with patch.object(desktop.terminal, "pid_running", return_value=False):
                desktop.verify_cancelled_children(receipts, sentinel, "now")
            with patch.object(desktop.terminal, "pid_running", side_effect=lambda pid: pid == 8), \
                    patch.object(desktop.time, "monotonic", side_effect=[0, 0, 11]), \
                    patch.object(desktop.time, "sleep"):
                with self.assertRaisesRegex(RuntimeError, "survived"):
                    desktop.verify_cancelled_children(receipts, sentinel, "now")
            sentinel.poll.return_value = 1
            with patch.object(desktop.terminal, "pid_running", return_value=False):
                with self.assertRaisesRegex(RuntimeError, "sentinel exited"):
                    desktop.verify_cancelled_children(receipts, sentinel, "now")
            with self.assertRaisesRegex(RuntimeError, "exactly one receipt"):
                desktop.verify_cancelled_children(receipts, sentinel, "missing")


if __name__ == "__main__":
    unittest.main()
