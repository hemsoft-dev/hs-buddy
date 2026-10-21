import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("ralph_desktop", Path(__file__).with_name("ralph-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def receipt(**changes):
    value = dict(mode="quick", max=3, model="", noAudio=True, autopilot=True, agents=[], cwd="", pid=10, child=None)
    value.update(changes)
    return value


class RalphDesktopTests(unittest.TestCase):
    def test_non_runner_is_refused_before_argument_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()

    def test_the_stub_loop_runs_like_the_real_script_and_leaves_the_repository_alone(self):
        pwsh = shutil.which("pwsh")
        self.assertIsNotNone(pwsh, "PowerShell 7 is required to validate the Ralph stub")
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            scripts = desktop.stub_scripts(root / "scripts")
            for name in ("ralph.ps1", "ralph-pr.ps1", "ralph-issues.ps1", "config/models.json", "config/agents.json"):
                self.assertTrue((scripts / name).is_file(), name)
            tools = desktop.stub_tools(root / "tools")
            repository = root / f"repo with spaces {desktop.UNICODE}"
            head = desktop.repository(repository)
            proofs = root / "starts"
            proofs.mkdir()
            prompt = root / "prompt.md"
            prompt.write_text("fail", encoding="utf-8")
            environment = dict(os.environ, BUDDY_RALPH_PROOF=str(proofs),
                PATH=str(tools) + os.pathsep + os.environ["PATH"])
            flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
            result = subprocess.run([pwsh, "-NoProfile", "-NonInteractive", "-File", str(scripts / "ralph.ps1"),
                "-Autopilot", "-Max", "3", "-Prompt", str(prompt), "-NoAudio"], cwd=repository, env=environment,
                capture_output=True, timeout=120, creationflags=flags)
            output = result.stdout.decode("utf-8", "replace")
            self.assertEqual(result.returncode, 3, result.stderr.decode("utf-8", "replace"))
            self.assertIn("== Iteration 1/3", output)
            self.assertIn("RALPH STUB TOOL stub-copilot 1.0", output)
            self.assertIn("RALPH STUB FAILING", output)
            desktop.verify_receipts(proofs, repository, dict(fail=1))
            with self.assertRaisesRegex(RuntimeError, "Expected loop starts"):
                desktop.verify_receipts(proofs, repository, dict(fail=2))
            desktop.verify_repository(repository, head)
            (repository / "new.txt").write_text("changed", encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "changed the repository"):
                desktop.verify_repository(repository, head)

    def test_receipts_must_show_the_repository_and_launch_defaults(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            for index, value in enumerate([receipt(cwd=str(root)), receipt(cwd=str(root), noAudio=False)]):
                (root / f"start-{index}.json").write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "defaults"):
                desktop.verify_receipts(root, root, dict(quick=2))
            (root / "start-1.json").write_text(json.dumps(receipt(cwd=str(root / "elsewhere"))), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "its repository"):
                desktop.verify_receipts(root, root, dict(quick=2))

    def test_a_surviving_loop_process_fails_the_stop_check(self):
        sentinel = Mock()
        sentinel.poll.return_value = None
        with patch.object(desktop.terminal, "pid_running", return_value=False):
            desktop.verify_stopped(receipt(pid=7, child=8), sentinel)
        with patch.object(desktop.terminal, "pid_running", side_effect=lambda pid: pid == 8), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 0, 16]), \
                patch.object(desktop.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "survived"):
                desktop.verify_stopped(receipt(pid=7, child=8), sentinel)
        with self.assertRaisesRegex(RuntimeError, "Invalid loop process receipt"):
            desktop.verify_stopped(receipt(pid=7, child=None), sentinel)


if __name__ == "__main__":
    unittest.main()
