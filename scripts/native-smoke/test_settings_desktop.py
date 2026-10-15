"""Choose supported fixtures before requesting sizes; never accept clamping."""
import importlib.util
import json
import os
from pathlib import Path
import struct
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


class ColorReadVerification(unittest.TestCase):
    def capture(self):
        path = Mock()
        path.read_bytes.return_value = bytes(16) + struct.pack(">II", 1200, 820)
        path.with_suffix.return_value.read_text.return_value = json.dumps(dict(capturePixelsPerWindowPoint=1))
        return path

    def test_wrong_hex_and_misread_glyphs_are_never_accepted_as_the_expected_color(self):
        label = dict(left=72, save=dict(left=1134, top=367, height=10))
        for actual in ("#8246a1", "#8246al"):
            with self.subTest(actual=actual), patch.object(desktop.ocr, "locate_image", return_value=label), \
                    patch.object(desktop, "run", side_effect=["", actual, actual]):
                with self.assertRaisesRegex(RuntimeError, "exact saved hex"):
                    desktop.verify_color(self.capture(), "#8246af", "macos")

    def test_a_second_line_mode_must_read_the_exact_value(self):
        label = dict(left=72, save=dict(left=1134, top=367, height=10))
        with patch.object(desktop.ocr, "locate_image", return_value=label), \
                patch.object(desktop, "run", side_effect=["", "#8246al", "#8246af"]):
            desktop.verify_color(self.capture(), "#8246af", "macos")


class DraftReadVerification(unittest.TestCase):
    capture = ColorReadVerification.capture

    def test_wrong_glyphs_and_additional_text_are_rejected(self):
        label = dict(left=73, save=dict(left=1133, top=281, height=11))
        for actual in ("draft audlo", "draft audio extra", "; draft audio ;"):
            with self.subTest(actual=actual), patch.object(desktop.ocr, "locate_image", return_value=label), \
                    patch.object(desktop, "run", side_effect=["", actual, actual]):
                with self.assertRaisesRegex(RuntimeError, "exact retained draft"):
                    desktop.verify_draft(self.capture(), "Notification audio file", "draft audio")

    def test_second_mode_requires_the_exact_unaliased_value(self):
        label = dict(left=73, save=dict(left=1133, top=281, height=11))
        with patch.object(desktop.ocr, "locate_image", return_value=label), \
                patch.object(desktop, "run", side_effect=["", "draft audlo", "draft audio"]):
            desktop.verify_draft(self.capture(), "Notification audio file", "draft audio")


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

    def test_text_replacement_never_types_into_another_frontmost_app(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123)
        subprocess_module = desktop.OwnedMacInput.require_focus.__globals__["subprocess"]
        with patch.object(subprocess_module, "run", return_value=Mock(stdout="456\n")) as run:
            with self.assertRaisesRegex(RuntimeError, "must be frontmost"):
                helper.replace_text("owned fixture text")
            self.assertEqual(run.call_count, 1)  # Only the focus query ran.

    def test_focus_diagnostics_never_read_another_frontmost_apps_ui(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123)
        subprocess_module = desktop.OwnedMacInput.require_focus.__globals__["subprocess"]
        with patch.object(subprocess_module, "run", return_value=Mock(stdout="456\n")) as run:
            with self.assertRaisesRegex(RuntimeError, "must be frontmost"):
                helper.focus_state()
            self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()
