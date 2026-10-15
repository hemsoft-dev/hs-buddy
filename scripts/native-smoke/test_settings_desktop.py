"""Choose supported fixtures before requesting sizes; never accept clamping."""
import importlib.util
import os
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("settings_desktop", Path(__file__).with_name("settings-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class DesktopLimits(unittest.TestCase):
    def test_full_and_small_host_limits_are_used_before_resize(self):
        self.assertEqual(desktop.planned_large_size(1920, 1080), (1200, 820))
        self.assertEqual(desktop.planned_large_size(1044, 788), (1044, 788))
        self.assertEqual(desktop.planned_large_size(1024, 683), (1024, 683))

    def test_a_host_below_the_supported_minimum_fails(self):
        with self.assertRaises(RuntimeError):
            desktop.planned_large_size(899, 820)
        with self.assertRaises(RuntimeError):
            desktop.planned_large_size(1200, 649)


class MacInputOwnership(unittest.TestCase):
    def test_normal_environment_cannot_load_input_frameworks(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "", "BUDDY_SETTINGS_FIXTURE": ""}):
            with self.assertRaisesRegex(RuntimeError, "disposable CI fixture"):
                desktop.OwnedMacInput(Mock(pid=123))

    def test_exited_owned_app_cannot_receive_input(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1"}):
            with self.assertRaisesRegex(RuntimeError, "live owned app"):
                desktop.OwnedMacInput(Mock(pid=123, poll=Mock(return_value=0)))

    def test_another_frontmost_app_is_rejected(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123)
        subprocess_module = desktop.OwnedMacInput.require_focus.__globals__["subprocess"]
        with patch.object(subprocess_module, "run", return_value=Mock(stdout="456\n")):
            with self.assertRaisesRegex(RuntimeError, "must be frontmost"):
                helper.require_focus()


if __name__ == "__main__":
    unittest.main()
