import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("settings_ocr", Path(__file__).with_name("settings-ocr.py"))
ocr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ocr)


def word(text, left, top, line):
    return dict(text=text, left=left, top=top, width=30, height=12, line=line)


class LabelGuards(unittest.TestCase):
    def test_utf8_tesseract_output_is_independent_of_a_windows_code_page(self):
        output = ("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n"
            "5\t1\t24\t1\t1\t1\t506\t387\t24\t10\t90\t‘Add\n"
            "5\t1\t24\t1\t1\t2\t535\t388\t48\t10\t90\taccount\n").encode("utf-8")
        real_run = subprocess.run
        def emit_tsv(_arguments, **kwargs):
            # Emulate the Windows default only when the caller leaves encoding
            # implicit. The real child emits the exact UTF-8 punctuation that
            # the failed CI capture decoded as mojibake.
            kwargs.setdefault("encoding", "cp1252")
            return real_run([sys.executable, "-c", f"import sys; sys.stdout.buffer.write({output!r})"], **kwargs)
        with patch.object(ocr.shutil, "which", return_value="tesseract"), patch.object(ocr.subprocess, "run", side_effect=emit_tsv):
            words = ocr.recognize(Path("owned-capture.png"))
        self.assertEqual(words[0]["text"], "‘Add")
        self.assertEqual(ocr.locate_label(words, "Add account")["left"], 506)

    def test_model_field_does_not_match_premium_model_label(self):
        words = [word("Model", 80, 200, "default"), word("ID", 120, 200, "default"),
            word("Premium", 80, 300, "premium"), word("model", 120, 300, "premium"), word("ID", 160, 300, "premium")]
        self.assertEqual(ocr.find_label(words, "Model ID", field=True)["top"], 200)

    def test_missing_model_label_cannot_select_the_premium_field(self):
        words = [word("Premium", 80, 300, "premium"), word("model", 120, 300, "premium"), word("ID", 160, 300, "premium")]
        with self.assertRaises(RuntimeError):
            ocr.find_label(words, "Model ID", field=True)

    def test_a_joined_ocr_word_matches_the_exact_label(self):
        words = [word("ModelID", 80, 200, "default"), word("Save", 1100, 200, "default")]
        self.assertEqual(ocr.find_label(words, "Model ID", field=True)["top"], 200)

    def test_model_id_ocr_variants_still_exclude_the_premium_suffix(self):
        words = [word("Model", 80, 200, "default"), word("1D", 120, 200, "default"),
            word("Premium", 80, 300, "premium"), word("model", 120, 300, "premium"), word("1D", 160, 300, "premium")]
        labels = "Model ID|Model 1D|Model lD"
        self.assertEqual(ocr.locate_label(words, labels, field=True)["top"], 200)
        with self.assertRaises(RuntimeError):
            ocr.locate_label(words[2:], labels, field=True)

    def test_account_columns_are_separate_field_label_groups(self):
        words = [word("GitHiub", 80, 300, "row"), word("username", 120, 300, "row"),
            word("Organization", 640, 300, "row")]
        self.assertEqual(ocr.find_label(words, "Organization", field=True)["left"], 640)

    def test_a_sidebar_ocr_glyph_does_not_prefix_the_model_label(self):
        words = [word("x", 24, 200, "row"), word("Model", 80, 200, "row"), word("ID", 120, 200, "row")]
        self.assertEqual(ocr.find_label(words, "Model ID", field=True)["left"], 80)

    def test_fields_exclude_words_left_of_the_settings_content(self):
        words = [word("Settings", 80, 100, "heading"), word("x", 45, 200, "row"),
            word("Model", 80, 200, "row"), word("ID", 120, 200, "row")]
        self.assertEqual(ocr.find_label(words, "Model ID", field=True)["left"], 80)

    def test_optical_glyph_bounds_can_start_left_of_the_heading(self):
        words = [word("Settings", 81, 105, "heading"), word("Accent", 80, 336, "field"),
            word("color", 123, 335, "field")]
        self.assertEqual(ocr.find_label(words, "Accent color", field=True)["left"], 80)

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
