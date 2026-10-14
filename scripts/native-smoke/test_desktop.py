"""The visual gate rejects a blank window and a title/tab-only rendering."""

import unittest
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from desktop import fixture, require_dashboard


class DashboardGate(unittest.TestCase):
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
