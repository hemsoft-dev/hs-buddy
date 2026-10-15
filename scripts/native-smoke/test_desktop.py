"""The visual gate rejects a blank window and a title/tab-only rendering."""

import unittest
import json
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event
from desktop import fixture, recording, require_dashboard, run


class DashboardGate(unittest.TestCase):
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
                self.assertEqual(len([key for key in config["ui"] if key.endswith("Color") or key.startswith("bg") or key.startswith("statusBar")]), 6)

    def test_blank_and_shell_only_fail(self):
        for text in ("", "Buddy File Edit View Help Dashboard", "Customize 0.1.1"):
            with self.subTest(text=text), self.assertRaises(RuntimeError):
                require_dashboard(text, "0.1.1")

    def test_stale_version_fails(self):
        with self.assertRaises(RuntimeError):
            require_dashboard("Your Universal Productivity Companion Customize 0.1.1", "0.1.2")

    def test_body_and_current_version_pass(self):
        require_dashboard("Your Universal\nProductivity Companion Customize Version 0.1.2", "0.1.2")


if __name__ == "__main__":
    unittest.main()
