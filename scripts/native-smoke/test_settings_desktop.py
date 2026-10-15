"""Choose supported fixtures before requesting sizes; never accept clamping."""
import importlib.util
from pathlib import Path
import unittest

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


if __name__ == "__main__":
    unittest.main()
