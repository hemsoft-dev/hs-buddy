import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("settings_ocr", Path(__file__).with_name("settings-ocr.py"))
ocr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ocr)


def word(text, left, top, line):
    return dict(text=text, left=left, top=top, width=30, height=12, line=line)


class LabelGuards(unittest.TestCase):
    def test_navigation_excludes_a_same_named_account_provider(self):
        words = [word("Settings", 80, 100, "a"), word("Copilot", 320, 150, "b"), word("Copilot", 200, 300, "c")]
        self.assertEqual(ocr.find_label(words, "Copilot", True)["top"], 150)
        with self.assertRaises(RuntimeError):
            ocr.find_label(words, "Copilot")

    def test_phrase_bounds_use_only_the_requested_words(self):
        words = [word("GitHub", 80, 300, "a"), word("username", 120, 300, "a"), word("Organization", 640, 300, "a")]
        self.assertEqual(ocr.find_label(words, "github username"), dict(left=80, top=300, width=70, height=12))
        with self.assertRaises(RuntimeError):
            ocr.find_label(words, "Missing")

    def test_field_locator_does_not_match_a_description(self):
        words = [word(text, index * 40, 200, "body") for index, text in enumerate("Configure a GitHub username and organization here".split())]
        words += [word("Organization", 640, 300, "field")]
        self.assertEqual(ocr.find_label(words, "Organization", field=True)["top"], 300)

    def test_navigation_uses_the_heading_not_the_reload_button(self):
        words = [word("Settings", 80, 100, "heading"), word("Reload", 800, 100, "button"), word("synced", 840, 100, "button"), word("settings", 880, 100, "button"), word("Accounts", 80, 150, "nav")]
        self.assertEqual(ocr.find_label(words, "Accounts", navigation=True)["top"], 150)


if __name__ == "__main__":
    unittest.main()
