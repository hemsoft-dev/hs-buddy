"""The visual gate rejects a blank window and a title/tab-only rendering."""

import unittest
import json
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event
from unittest.mock import patch
from desktop import fixture, inspect_screenshot, owned_wayland_node, recording, require_dashboard, require_size, run

BODY = "Your Universal Productivity Companion Customize Command Center Configure Accounts"


class DashboardGate(unittest.TestCase):
    def test_partial_ocr_results_cannot_be_combined_to_qualify(self):
        with TemporaryDirectory() as temporary, patch("desktop.run", side_effect=[
                f"{BODY} 0.1.20", "Version 0.1.2", BODY]) as recognize:
            with self.assertRaises(RuntimeError):
                inspect_screenshot(Path(temporary) / "dashboard.png", "0.1.2")
            self.assertEqual(recognize.call_count, 3)

    def test_fallback_requires_a_complete_current_result_and_stops_on_success(self):
        with TemporaryDirectory() as temporary, patch("desktop.run", side_effect=[
                f"{BODY} 0.1.20", f"{BODY} 0.1.2"]) as recognize:
            path = Path(temporary) / "dashboard.png"
            inspect_screenshot(path, "0.1.2")
            self.assertEqual(recognize.call_count, 2)
            self.assertEqual(json.loads(path.with_suffix(".ocr-mode.json").read_text())["mode"], 3)
            self.assertEqual(path.with_suffix(".ocr.txt").read_text(), f"{BODY} 0.1.2")

    def test_ignored_or_clamped_resize_fails(self):
        for actual in (dict(width=1200, height=820), dict(width=900, height=600),
                       dict(width=float("nan"), height=650), dict(width=900, height=float("inf"))):
            with self.assertRaises(RuntimeError):
                require_size(actual, 900, 650)
        require_size(dict(width=900, height=650), 900, 650)

    def test_wayland_capture_cannot_select_another_process_or_window(self):
        ours = dict(pid=42, app_id="buddy-native", id=7, rect=dict(x=80, y=90, width=900, height=650))
        other = dict(pid=43, app_id="buddy-native", id=8)
        tree = dict(nodes=[dict(nodes=[other])], floating_nodes=[ours])
        self.assertEqual(owned_wayland_node(tree, 42, 7)["rect"], dict(x=80, y=90, width=900, height=650))
        self.assertIsNone(owned_wayland_node(tree, 42, 8))
        self.assertIsNone(owned_wayland_node(tree, 99))

    def test_header_without_the_card_body_cannot_qualify(self):
        for text in ("Your Universal Productivity Companion Customize 0.1.2",
                     "Your Universal Productivity Companion Customize Command Center 0.1.2"):
            with self.assertRaises(RuntimeError):
                require_dashboard(text, "0.1.2")
    def test_command_timeout_can_be_set_by_the_caller(self):
        self.assertEqual(run(sys.executable, "-c", "print('owned child')", timeout=5), "owned child")
        with self.assertRaises(subprocess.TimeoutExpired):
            run(sys.executable, "-c", "import time; time.sleep(10)", timeout=0.1)

    def recording_storage_failure(self, action_error):
        started, release = Event(), Event()
        calls = []
        class StorageFault:
            def screenshot(self, path):
                calls.append(path)
                started.set()
                if not release.wait(10):
                    raise TimeoutError("Storage fixture was not released")
                path.write_bytes(b"owned fault-injection data")
        with TemporaryDirectory() as temporary:
            frames = Path(temporary) / "frames"
            try:
                with recording(StorageFault(), frames):
                    self.assertTrue(started.wait(10))
                    # A real filesystem failure in the pending screenshot write.
                    frames.rmdir()
                    release.set()
                    if action_error:
                        raise action_error
            except BaseException as observed:
                self.assertEqual(len(calls), 1, "A failed capture must not replay")
                return observed
            self.fail("The failed required recording must fail qualification")

    def test_recording_write_failure_keeps_original_action_exception(self):
        primary = ValueError("owned primary action failure")
        self.assertIs(self.recording_storage_failure(primary), primary)

    def test_recording_write_failure_fails_an_otherwise_successful_action(self):
        self.assertIsInstance(self.recording_storage_failure(None), FileNotFoundError)

    def test_theme_fixtures_include_the_persisted_electron_color_presets(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            light = json.loads(fixture(root / "light", "light").read_text())
            dark = json.loads(fixture(root / "dark", "dark").read_text())
            self.assertEqual(light["ui"]["bgPrimary"], "#ffffff")
            self.assertEqual(light["ui"]["fontColor"], "#1f1f1f")
            self.assertEqual(dark["ui"]["bgPrimary"], "#1e1e1e")
            self.assertEqual(dark["ui"]["fontColor"], "#cccccc")
            for config in (light, dark):
                self.assertTrue(config["ui"]["dashboardCards"]["command-center"])
                self.assertEqual(len([key for key in config["ui"] if key.endswith("Color") or key.startswith("bg") or key.startswith("statusBar")]), 6)

    def test_blank_and_shell_only_fail(self):
        for text in ("", "Buddy File Edit View Help Dashboard", "Customize 0.1.1"):
            with self.subTest(text=text), self.assertRaises(RuntimeError):
                require_dashboard(text, "0.1.1")

    def test_stale_version_fails(self):
        with self.assertRaises(RuntimeError):
            require_dashboard(f"{BODY} 0.1.1", "0.1.2")

    def test_a_version_prefix_cannot_qualify_a_different_release(self):
        for observed in ("0.1.20", "10.1.2", "0.1.2.1"):
            with self.subTest(observed=observed), self.assertRaises(RuntimeError):
                require_dashboard(f"{BODY} {observed}", "0.1.2")

    def test_body_and_current_version_pass(self):
        require_dashboard(f"{BODY} Version 0.1.2", "0.1.2")


if __name__ == "__main__":
    unittest.main()
