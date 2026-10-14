"""The visual gate rejects a blank window and a title/tab-only rendering."""

import unittest
from desktop import require_dashboard


class DashboardGate(unittest.TestCase):
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
