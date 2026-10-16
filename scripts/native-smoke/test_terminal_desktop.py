import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("terminal_desktop", Path(__file__).with_name("terminal-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class TerminalDesktopOwnershipTests(unittest.TestCase):
    def test_non_runner_is_refused_before_argument_file_backend_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser, \
                patch.object(desktop.settings, "SettingsBackend") as backend:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()
            backend.assert_not_called()

    def test_output_must_be_fresh_and_inside_the_absolute_runner_root(self):
        with tempfile.TemporaryDirectory() as root:
            with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": root}):
                owned = Path(root) / "new-evidence"
                self.assertEqual(desktop.owned_output(owned), owned.resolve())
                for rejected in (root, Path(root).parent / "unowned"):
                    with self.assertRaisesRegex(RuntimeError, "fresh"):
                        desktop.owned_output(rejected)
                owned.mkdir()
                with self.assertRaisesRegex(RuntimeError, "fresh"):
                    desktop.owned_output(owned)

    def test_old_command_proof_cannot_satisfy_a_new_invocation(self):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root)
            workspace = output / "workspace with spaces 資料"
            body = dict(run="old-run", cwd=str(workspace), unicode="資料界é")
            (output / "command-proof.json").write_text(json.dumps(body), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "current terminal command"):
                desktop.verify_proof(output, "new-run", workspace)
            body["run"] = "new-run"
            (output / "command-proof.json").write_text(json.dumps(body), encoding="utf-8")
            desktop.verify_proof(output, "new-run", workspace)
            body["cwd"] = str(workspace.parent)
            (output / "command-proof.json").write_text(json.dumps(body), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "exact Unicode"):
                desktop.verify_proof(output, "new-run", workspace)

    def test_cleanup_failure_keeps_the_original_product_failure_and_fails_an_otherwise_good_run(self):
        child = Mock()
        child.poll.return_value = None
        cleanup = RuntimeError("actual terminate failure")
        child.terminate.side_effect = cleanup
        primary = ValueError("original product failure")
        desktop.stop_sentinel(child, primary)
        self.assertEqual(primary.__notes__, ["Owned sentinel cleanup also failed: RuntimeError"])
        with self.assertRaises(RuntimeError) as caught:
            desktop.stop_sentinel(child, None)
        self.assertIs(caught.exception, cleanup)

    def test_child_completion_cannot_use_another_invocations_old_receipt(self):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root)
            path = output / "owned-children.jsonl"
            path.write_text(json.dumps(dict(run="old", pids=[10, 20])) + "\n", encoding="utf-8")
            sentinel = Mock()
            sentinel.poll.return_value = None
            with patch.object(desktop, "pid_running", return_value=False) as observe:
                with self.assertRaisesRegex(RuntimeError, "this command invocation"):
                    desktop.verify_children(output, sentinel, "fresh")
                observe.assert_not_called()
                path.write_text(json.dumps(dict(run="fresh", pids=[10, 20])) + "\n", encoding="utf-8")
                desktop.verify_children(output, sentinel, "fresh")
                self.assertEqual([call.args for call in observe.call_args_list], [(10,), (20,)])
                sentinel.poll.assert_called_once()


if __name__ == "__main__":
    unittest.main()
