import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("terminal_transport", Path(__file__).with_name("terminal-transport.py"))
transport = importlib.util.module_from_spec(spec)
spec.loader.exec_module(transport)


class TerminalTransportOwnershipTests(unittest.TestCase):
    def test_non_runner_is_rejected_before_binary_or_workspace_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}), patch.object(transport.subprocess, "check_output") as process:
            with self.assertRaisesRegex(RuntimeError, "ephemeral"):
                transport.main()
            process.assert_not_called()

    def test_missing_or_relative_runner_root_is_rejected(self):
        for root in ("", "relative"):
            with self.subTest(root=root), patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_TEMP": root}):
                with self.assertRaisesRegex(RuntimeError, "absolute owned"):
                    transport.runner_root()

    def test_real_absolute_runner_root_is_retained_without_replacing_environment(self):
        with tempfile.TemporaryDirectory() as root, patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUNNER_TEMP": root}):
            self.assertEqual(transport.runner_root(), Path(root).resolve())
            self.assertEqual(os.environ["RUNNER_TEMP"], root)


if __name__ == "__main__":
    unittest.main()
