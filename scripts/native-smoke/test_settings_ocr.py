import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("settings_ocr", Path(__file__).with_name("settings-ocr.py"))
ocr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ocr)


def word(text, left, top, line):
    return dict(text=text, left=left, top=top, width=30, height=12, line=line)


class SessionInputRow(unittest.TestCase):
    def test_actual_failed_intel_layout_measures_the_editor_row_below_the_old_click(self):
        evidence = json.loads(Path(__file__).with_name('fixtures').joinpath('session-input-row-ocr.json').read_text(encoding='utf-8'))
        label = ocr.locate_label(evidence['words'], 'VS Code User directory', field=True)
        control = ocr.aligned_control(evidence['words'], label, 'Scan folder')
        self.assertEqual(label, dict(left=65, top=177, width=135, height=13))
        self.assertEqual(control, dict(left=1103, top=214, width=70, height=11))
        self.assertEqual(label['top'] + label['height'] + 20, 210)
        self.assertEqual(control['top'] + control['height'] / 2, 219.5)

    def test_complete_adjacent_button_locates_the_editor_row_at_both_capture_scales(self):
        for scale in (1, 2):
            with self.subTest(scale=scale):
                label = dict(left=65*scale, top=177*scale, width=135*scale, height=13*scale)
                words = [dict(text='Scan', left=1103*scale, top=215*scale, width=29*scale, height=10*scale, line='scan'),
                    dict(text='folder', left=1138*scale, top=215*scale, width=35*scale, height=10*scale, line='scan')]
                self.assertEqual(ocr.aligned_control(words, label, 'Scan folder'),
                    dict(left=1103*scale, top=215*scale, width=70*scale, height=10*scale))

    def test_competing_buttons_in_the_same_row_are_rejected(self):
        label = dict(left=65, top=177, width=135, height=13)
        words = [dict(text='Scan folder', left=x, top=215, width=70, height=10, line=str(x)) for x in (600, 1103)]
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.aligned_control(words, label, 'Scan folder')

    def test_button_on_another_row_or_partial_label_is_rejected(self):
        label = dict(left=65, top=177, width=135, height=13)
        for text, top in (('Scan folder', 300), ('Scan folder', 160), ('Scan', 215)):
            with self.subTest(text=text, top=top), self.assertRaises(RuntimeError):
                ocr.aligned_control([dict(text=text, left=1103, top=top, width=70, height=10, line='scan')], label, 'Scan folder')


class LabelGuards(unittest.TestCase):
    def test_workspace_rename_is_bounded_by_the_two_visible_field_labels(self):
        evidence = json.loads(Path(__file__).with_name('fixtures').joinpath('terminal-workspace-rename-words.json').read_text())
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(evidence['block'], 'Rename', content=True)
        # Sparse OCR sees both row anchors but misreads the File menu. The
        # explicit field scope needs neither a menu nor a mixed-engine result.
        with self.assertRaisesRegex(RuntimeError, 'menu row'):
            ocr.find_label(evidence['sparse'], 'Rename', content=True)
        self.assertEqual(ocr.find_label(evidence['sparse'], 'Rename', between=('Workspace name', 'Selected shell name')),
            dict(left=830, top=172, width=49, height=10))
        with self.assertRaises(RuntimeError):
            ocr.find_label(evidence['block'], 'Rename', between=('Workspace name', 'Selected shell name'))

    def test_bounded_row_never_falls_back_to_a_neighbor_or_hides_a_duplicate(self):
        words = [word('Workspace name', 60, 140, 'start'), word('Rename', 830, 172, 'workspace'),
            word('Selected shell name', 60, 208, 'end'), word('Rename', 932, 233, 'shell')]
        for missing in ('Workspace name', 'Selected shell name', 'Rename'):
            kept = [w for w in words if not (w['text'] == missing and (missing != 'Rename' or w['top'] == 172))]
            with self.subTest(missing=missing), self.assertRaises(RuntimeError):
                ocr.find_label(kept, 'Rename', between=('Workspace name', 'Selected shell name'))
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(words + [word('Rename', 500, 172, 'duplicate')], 'Rename',
                between=('Workspace name', 'Selected shell name'))
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(words + [word('Workspace name', 400, 140, 'duplicate-anchor')], 'Rename',
                between=('Workspace name', 'Selected shell name'))
        with self.assertRaisesRegex(RuntimeError, 'out of order'):
            ocr.find_label(words, 'Rename', between=('Selected shell name', 'Workspace name'))

    def test_border_punctuation_does_not_create_two_matches_for_one_interrupt_button(self):
        words = [dict(text="Interrupt", left=499, top=151, width=61, height=30, line="16/1/1"),
            dict(text="(", left=584, top=155, width=4, height=30, line="16/1/1"),
            dict(text="Close", left=595, top=163, width=37, height=11, line="16/1/1"),
            dict(text="shell", left=637, top=163, width=31, height=11, line="16/1/1")]
        self.assertEqual(ocr.find_label(words, "Interrupt"), dict(left=499, top=151, width=61, height=30))

    def test_punctuation_filter_never_combines_competing_controls(self):
        words = [word("Interrupt", 140, 188, "one"), word("(", 172, 188, "one"),
            word("Interrupt", 400, 188, "two"), word(")", 432, 188, "two")]
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(words, "Interrupt")

    def test_macos_ocr_runs_the_declared_compiled_helper_without_swift_interpretation(self):
        with tempfile.TemporaryDirectory() as folder:
            helper, image = Path(folder) / "buddy-ocr-words", Path(folder) / "owned.png"
            helper.write_bytes(b"synthetic prepared helper")
            with patch.dict(os.environ, {"BUDDY_SETTINGS_OCR_HELPER": str(helper)}), \
                    patch.object(ocr.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "[]", "")) as run:
                self.assertEqual(ocr.recognize_macos(image), [])
            self.assertEqual(run.call_args.args[0], [str(helper), str(image)])
            self.assertEqual(run.call_args.kwargs["timeout"], 30)

    def test_missing_prepared_macos_helper_fails_before_spawning_a_process(self):
        with tempfile.TemporaryDirectory() as folder:
            with patch.dict(os.environ, {"BUDDY_SETTINGS_OCR_HELPER": str(Path(folder) / "missing")}), \
                    patch.object(ocr.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "[]", "")) as run:
                with self.assertRaisesRegex(RuntimeError, "compiled"):
                    ocr.recognize_macos(Path(folder) / "owned.png")
            run.assert_not_called()

    def test_macos_native_ocr_uses_the_untouched_capture_and_same_label_rules(self):
        native = [dict(text="Run and save", left=197, top=760, width=86, height=13, line="save")]
        with tempfile.TemporaryDirectory() as folder:
            image = Path(folder) / "owned.png"
            image.write_bytes(b"untouched original capture")
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr.sys, "platform", "darwin"), \
                    patch.object(ocr, "recognize", return_value=[]), \
                    patch.object(ocr, "recognize_macos", return_value=native) as recognize_native, \
                    patch.object(ocr.subprocess, "run"):
                self.assertEqual(ocr.locate_image(image, "Run and save"),
                    dict(left=197, top=760, width=86, height=13))
            recognize_native.assert_called_once_with(image)
            self.assertEqual(image.read_bytes(), b"untouched original capture")

    def test_macos_native_fallback_cannot_select_between_competing_controls(self):
        duplicate = [word("Refresh", 140, 188, "one"), word("Refresh", 400, 188, "two")]
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr.sys, "platform", "darwin"), \
                    patch.object(ocr, "recognize", return_value=[]), \
                    patch.object(ocr, "recognize_macos", return_value=duplicate) as recognize_native, \
                    patch.object(ocr.subprocess, "run"):
                with self.assertRaisesRegex(RuntimeError, "unambiguously"):
                    ocr.locate_image(Path(folder) / "owned.png", "Refresh")
            recognize_native.assert_called_once()

    def test_windows_native_ocr_recovers_a_legible_button_missed_by_tesseract(self):
        missed = [word("response", 359, 724, "cancel")]
        native = [dict(text="Run and save", left=201, top=721, width=81, height=10, line="save")]
        with tempfile.TemporaryDirectory() as folder:
            image = Path(folder) / "owned.png"
            image.write_bytes(b"untouched original capture")
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr.sys, "platform", "win32"), \
                    patch.object(ocr, "recognize", return_value=missed), \
                    patch.object(ocr, "recognize_windows", return_value=native, create=True) as recognize_native, \
                    patch.object(ocr.subprocess, "run"):
                self.assertEqual(ocr.locate_image(image, "Run and save"),
                    dict(left=201, top=721, width=81, height=10))
            recognize_native.assert_called_once_with(image)
            self.assertEqual(image.read_bytes(), b"untouched original capture")

    def test_an_ambiguous_engine_result_cannot_be_replaced_by_a_later_unique_result(self):
        duplicate = [word("Refresh", 140, 188, "one"), word("Refresh", 400, 188, "two")]
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr.sys, "platform", "win32"), \
                    patch.object(ocr, "recognize", return_value=duplicate) as recognize, \
                    patch.object(ocr, "recognize_windows", return_value=duplicate[:1], create=True) as recognize_native, \
                    patch.object(ocr.subprocess, "run"):
                with self.assertRaisesRegex(RuntimeError, "unambiguously"):
                    ocr.locate_image(Path(folder) / "owned.png", "Refresh")
            self.assertEqual(recognize.call_count, 1)
            recognize_native.assert_not_called()

    def test_windows_native_fallback_keeps_competing_controls_ambiguous(self):
        duplicate = [word("Refresh", 140, 188, "one"), word("Refresh", 400, 188, "two")]
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr.sys, "platform", "win32"), \
                    patch.object(ocr, "recognize", return_value=[]), \
                    patch.object(ocr, "recognize_windows", return_value=duplicate, create=True) as recognize_native, \
                    patch.object(ocr.subprocess, "run"):
                with self.assertRaisesRegex(RuntimeError, "unambiguously"):
                    ocr.locate_image(Path(folder) / "owned.png", "Refresh")
            recognize_native.assert_called_once()

    def test_non_windows_hosts_never_invoke_the_windows_fallback(self):
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr.sys, "platform", "linux"), \
                    patch.object(ocr, "recognize", return_value=[]), \
                    patch.object(ocr, "recognize_windows", create=True) as recognize_native, \
                    patch.object(ocr.subprocess, "run"):
                with self.assertRaisesRegex(RuntimeError, "unambiguously"):
                    ocr.locate_image(Path(folder) / "owned.png", "Refresh")
            recognize_native.assert_not_called()

    def test_tesseract_literal_quotes_do_not_consume_following_tsv_rows(self):
        output = ("level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n"
            "5\t1\t1\t1\t1\t1\t10\t10\t8\t10\t90\t\"y\n"
            "5\t1\t2\t1\t1\t1\t100\t50\t30\t12\t90\tSave\n"
            "5\t1\t2\t1\t1\t2\t136\t50\t60\t12\t90\tdirectory\n")
        with patch.object(ocr.shutil, "which", return_value="tesseract"), patch.object(ocr.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, stdout=output)):
            words = ocr.recognize(Path("owned-capture.png"))
        self.assertEqual([item["text"] for item in words], ['"y', "Save", "directory"])
        self.assertEqual(ocr.locate_label(words, "Save directory")["left"], 100)

    def test_scaled_label_analysis_returns_original_window_coordinates_and_keeps_ambiguity_guards(self):
        scaled = [dict(text="Save", left=300, top=600, width=90, height=36, line="1"),
            dict(text="directory", left=408, top=600, width=180, height=36, line="1")]
        with tempfile.TemporaryDirectory() as folder:
            image = Path(folder) / "owned.png"
            image.write_bytes(b"untouched original capture")
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr, "recognize", side_effect=[[], [], [], scaled]) as recognize, \
                    patch.object(ocr.subprocess, "run") as scale:
                self.assertEqual(ocr.locate_image(image, "Save directory"), dict(left=100, top=200, width=96, height=12))
            self.assertEqual(image.read_bytes(), b"untouched original capture")
            self.assertIn("scale=iw*3:ih*3:flags=neighbor", scale.call_args.args[0])
            self.assertEqual(recognize.call_args.args[1], 11)
            duplicate = scaled + [dict(text="Save", left=900, top=600, width=90, height=36, line="2"),
                dict(text="directory", left=1008, top=600, width=180, height=36, line="2")]
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr, "recognize", side_effect=[[], [], [], duplicate]), \
                    patch.object(ocr.subprocess, "run"):
                with self.assertRaisesRegex(RuntimeError, "unambiguously"):
                    ocr.locate_image(image, "Save directory")

    def test_light_text_on_a_colored_button_is_read_from_an_inverted_copy(self):
        inverted = [dict(text="Add", left=3438, top=282, width=78, height=33, line="1")]
        with tempfile.TemporaryDirectory() as folder:
            image = Path(folder) / "owned.png"
            image.write_bytes(b"untouched original capture")
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr, "recognize", side_effect=[[], [], [], [], inverted]), \
                    patch.object(ocr.subprocess, "run") as convert:
                self.assertEqual(ocr.locate_image(image, "Add"), dict(left=1146, top=94, width=26, height=11))
            self.assertEqual(image.read_bytes(), b"untouched original capture")
            filters = [call.args[0][call.args[0].index("-vf") + 1] for call in convert.call_args_list]
            self.assertEqual(filters, ["scale=iw*3:ih*3:flags=neighbor",
                "scale=iw*3:ih*3:flags=neighbor,format=gray,negate"])

    def test_a_button_sparse_segmentation_skips_is_read_from_a_uniform_block(self):
        block = [dict(text="+", left=3375, top=381, width=30, height=30, line="1/1/2"),
            dict(text="Add", left=3438, top=381, width=78, height=33, line="1/1/2")]
        with tempfile.TemporaryDirectory() as folder:
            image = Path(folder) / "owned.png"
            image.write_bytes(b"untouched original capture")
            with patch.object(ocr.shutil, "which", return_value="tesseract"), \
                    patch.object(ocr, "recognize", side_effect=[[], [], [], [], [], block]) as recognize, \
                    patch.object(ocr.subprocess, "run") as convert:
                self.assertEqual(ocr.locate_image(image, "Add"), dict(left=1146, top=127, width=26, height=11))
            self.assertEqual(image.read_bytes(), b"untouched original capture")
            self.assertEqual([call.args[1] for call in recognize.call_args_list], [11, 3, 6, 11, 11, 6])
            self.assertEqual(recognize.call_args.args[0], image.with_suffix(".ocr-analysis-3x.png"))
            # Each copy is converted once; the block pass rereads the same files.
            self.assertEqual(convert.call_count, 2)

    def test_button_label_wins_over_the_same_word_in_status_prose(self):
        words = [word("Refresh", 142, 188, "button"),
            word("Comment", 64, 282, "status"), word("posted.", 99, 282, "status"),
            word("Refresh", 134, 282, "status"), word("to", 169, 282, "status"),
            word("load", 204, 282, "status")]
        self.assertEqual(ocr.find_label(words, "Refresh")["top"], 188)

    def test_two_complete_button_labels_are_still_ambiguous(self):
        with self.assertRaises(RuntimeError):
            ocr.find_label([word("Refresh", 140, 188, "one"),
                word("Refresh", 400, 188, "two")], "Refresh")

    def test_tab_selection_requires_toolbar_context_and_an_unambiguous_row(self):
        with self.assertRaises(RuntimeError):
            ocr.find_label([word("Reviews", 200, 245, "2")], "Reviews", tab=True)
        words = [word("Filter", 200, 170, "1"), word("loaded", 235, 170, "1"),
            word("items", 270, 170, "1"), word("Reviews", 200, 245, "2"),
            word("Reviews", 400, 245, "2")]
        with self.assertRaises(RuntimeError):
            ocr.find_label(words, "Reviews", tab=True)

    def test_draft_region_excludes_focus_border_using_actual_glyph_bounds(self):
        self.assertEqual(ocr.input_region(dict(left=73, save=dict(left=1133, top=281, height=11)), (1200, 820)),
            dict(left=78, top=275, width=1028, height=23))
        self.assertEqual(ocr.input_region(dict(left=146, save=dict(left=2266, top=562, height=22)), (2400, 1640)),
            dict(left=155, top=551, width=2058, height=44))

    def test_draft_region_rejects_invalid_height_or_out_of_capture_coordinates(self):
        for height in (0, -1, float("nan")):
            with self.assertRaisesRegex(RuntimeError, "positive measured"):
                ocr.input_region(dict(left=73, save=dict(left=1133, top=281, height=height)), (1200, 820))
        with self.assertRaisesRegex(RuntimeError, "invalid capture bounds"):
            ocr.input_region(dict(left=73, save=dict(left=1133, top=281, height=11)), (900, 650))

    def test_color_analysis_region_preserves_the_measured_row_at_both_scales(self):
        label = dict(left=72, save=dict(left=1134, top=367, height=10))
        self.assertEqual(ocr.color_region(label, 1, (1200, 820)),
            dict(left=72, top=360, width=1042, height=24))
        scaled = dict(left=144, save=dict(left=2268, top=734, height=20))
        self.assertEqual(ocr.color_region(scaled, 2, (2400, 1640)),
            dict(left=144, top=720, width=2084, height=48))

    def test_color_region_cannot_escape_the_owned_capture(self):
        with self.assertRaisesRegex(RuntimeError, "invalid capture bounds"):
            ocr.color_region(dict(left=72, save=dict(left=1134, top=367, height=10)), 1, (900, 650))
        with self.assertRaisesRegex(RuntimeError, "positive measured"):
            ocr.color_region({}, float("nan"), (1200, 820))

    def test_mac_editor_row_uses_the_actual_save_baseline_not_label_padding(self):
        # Exact native Tesseract bounds from the failed Mac Accent capture.
        label = dict(left=72, top=333, width=72, height=10)
        save = dict(text="Save", left=1134, top=367, width=30, height=10, line="20/1/1")
        words = [save, word("Save", 1134, 444, "next-field")]
        actual = ocr.aligned_save(words, label)
        self.assertEqual(actual["top"] + actual["height"] / 2, 372)
        self.assertEqual(label["top"] + label["height"] + 20, 363)

    def test_a_missing_row_cannot_fall_back_to_the_next_fields_save(self):
        with self.assertRaisesRegex(RuntimeError, "not visible"):
            ocr.aligned_save([word("Save", 1134, 444, "next-field")],
                dict(left=72, top=333, width=72, height=10))

    def test_two_save_buttons_on_the_nearest_row_are_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "ambiguous"):
            ocr.aligned_save([word("Save", 600, 367, "a"), word("Save", 1134, 367, "b")],
                dict(left=72, top=333, width=72, height=10))

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

    def test_a_field_label_longer_than_five_words_is_still_a_field_label(self):
        # Windows OCR from native run 39139528913 read this six-word Tempo
        # label as one line; the five-word group guard rejected it.
        actual = [("Start", 72), ("time", 102), ("in", 132), ("the", 145), ("account", 168), ("timezone", 217)]
        words = [dict(text=text, left=left, top=620, width=26, height=13, line="22") for text, left in actual]
        label = ocr.find_label(words, "Start time in the account timezone", field=True)
        self.assertEqual(label, dict(left=72, top=620, width=171, height=13))
        # A longer prose line that merely starts with the label stays excluded.
        prose = words + [dict(text="here", left=260, top=620, width=26, height=13, line="22")]
        with self.assertRaises(RuntimeError):
            ocr.find_label(prose, "Start time in the account timezone", field=True)

    def test_navigation_uses_the_heading_not_the_reload_button(self):
        words = [word("Settings", 80, 100, "heading"), word("Reload", 800, 100, "button"), word("synced", 840, 100, "button"), word("settings", 880, 100, "button"), word("Accounts", 80, 150, "nav")]
        self.assertEqual(ocr.find_label(words, "Accounts", navigation=True)["top"], 150)


    def test_content_scope_excludes_global_tab_titles_at_each_capture_scale(self):
        for scale in (1, 1.5, 2, 3):
            for label in ("Jobs", "History", "Schedules", "Launch", "New job", "alpha", "Development"):
                with self.subTest(scale=scale, label=label):
                    words = [word("File", 100, 20, "menu"), word("Edit", 140, 20, "menu"),
                        word("View", 180, 20, "menu"), word("Help", 280, 20, "menu"),
                        word(label, 100, 55, "global-tab"), word(label, 100, 110, "view")]
                    for item in words:
                        for key in ("left", "top", "width", "height"):
                            item[key] *= scale
                    with self.assertRaises(ocr.AmbiguousLabel):
                        ocr.find_label(words, label)
                    self.assertEqual(ocr.find_label(words, label, content=True)["top"], 110 * scale)
                    with self.assertRaises(RuntimeError):
                        ocr.find_label(words[:-1], label, content=True)

    def test_settings_navigation_ignores_the_clipped_global_tab_heading(self):
        # Windows job 124177959929, settings/retained-drafts original OCR.
        words = json.loads(Path(__file__).with_name("fixtures").joinpath("settings-global-tab-words.json").read_text())
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(words, "Notifications", navigation=True)
        self.assertEqual(ocr.find_label(words, "Notifications", navigation=True, content=True),
            dict(left=471, top=159, width=75, height=10))

    def test_content_scope_anchors_the_menu_even_when_file_occurs_in_view_labels(self):
        words = [word("File", 100, 20, "menu"), word("Edit", 140, 20, "menu"),
            word("View", 180, 20, "menu"), word("Help", 280, 20, "menu"),
            word("Settings", 100, 55, "global-tab"), word("Settings", 100, 110, "heading"),
            word("Weather", 100, 150, "nav"), word("File", 100, 250, "view-file"),
            word("File", 100, 300, "another-file"), word("Help", 280, 300, "view-help")]
        self.assertEqual(ocr.find_label(words, "Weather", navigation=True, content=True)["top"], 150)
        # A second full menu must fail, rather than choose a row by order.
        words += [word(label, left, 350, "second-menu") for label, left
            in (("File",100),("Edit",140),("View",180),("Help",280))]
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(words, "Weather", navigation=True, content=True)

    def test_content_scope_still_rejects_competing_view_controls(self):
        words = [word("File", 100, 20, "menu"), word("Edit", 140, 20, "menu"),
                        word("View", 180, 20, "menu"), word("Help", 280, 20, "menu"),
            word("Jobs", 100, 55, "global-tab"), word("Jobs", 100, 110, "view"),
            word("Jobs", 300, 110, "another-control")]
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.find_label(words, "Jobs", content=True)
        with self.assertRaises(RuntimeError):
            ocr.find_label(words[2:], "Jobs", content=True)


if __name__ == "__main__":
    unittest.main()
