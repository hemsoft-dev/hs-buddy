"""Run the actual fresh lifecycle path with real OCR scoping, without an app."""
import contextlib
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("lifecycle_dashboard", HERE / "linux-installed-lifecycle.py")
lifecycle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lifecycle)
settings = lifecycle.load(HERE / "settings-desktop.py")


def words(scale=1):
    fixture = json.loads((HERE / "fixtures/linux-lifecycle-fresh-dashboard-words.json").read_text(encoding="utf-8"))
    return [dict(word, **{key: word[key] * scale for key in ("left", "top", "width", "height")}) for word in fixture["words"]]


def chrome_only(value):
    return [word for word in value if word["line"] in ("menu", "workspace", "footer")]


class FreshDashboardControls(unittest.TestCase):
    def fresh_pass(self, ready_words, capture_words=None):
        capture_words = ready_words if capture_words is None else capture_words
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            window = Mock()
            window.screenshot.side_effect = lambda path: path.write_bytes(b"synthetic OCR boundary, not a capture")
            @contextlib.contextmanager
            def running(_exe, _args, _environment, directory):
                directory.mkdir(parents=True)
                yield Mock(), window
            def recognize(path, mode=11):
                source = ready_words if path.name == "wait-status.png" else capture_words
                return copy.deepcopy(source[mode] if isinstance(source, dict) else source)
            with patch.object(lifecycle, "running", running), \
                    patch.object(lifecycle, "load", side_effect=lambda path: settings if path.name == "settings-desktop.py" else Mock()), \
                    patch.object(settings, "wait_visible", wraps=settings.wait_visible) as wait, \
                    patch.object(settings.ocr, "recognize", side_effect=recognize) as engine, \
                    patch.object(settings.ocr.shutil, "which", return_value=None), \
                    patch.object(settings.ocr, "analysis_images", side_effect=lambda path, _run: iter([path])), \
                    patch.object(settings.time, "monotonic", side_effect=[0, 16]), \
                    patch.object(settings.time, "sleep") as sleep:
                lifecycle.native_pass(Path("synthetic-app"), root / "profile", Mock(url="http://127.0.0.1:9"), root, "native-fresh", fresh=True)
                self.assertTrue(wait.call_args.kwargs["content"])
                self.assertEqual([call.args[0].name for call in window.screenshot.call_args_list], ["wait-status.png", "fresh.png"])
                sleep.assert_not_called()
                return json.loads((root / "native-fresh/fresh.dashboard-body.json").read_text()), engine.call_args_list

    def test_fresh_native_pass_requires_unconditional_dashboard_body_markers(self):
        for scale in (1, 2):
            with self.subTest(scale=scale):
                evidence, _ = self.fresh_pass(words(scale))
                self.assertEqual(evidence["markers"], ["Customize", "Your Universal Productivity Companion"])
                self.assertFalse(any(word["text"] == "Dashboard" for word in evidence["bodyWords"]))

    def test_tab_only_dashboard_never_proves_fresh_readiness(self):
        with self.assertRaisesRegex(RuntimeError, "expected status"):
            self.fresh_pass(chrome_only(words()))

    def test_capture_cannot_reuse_ready_frame_or_chrome_markers(self):
        source = words()
        for capture in (chrome_only(source),
                [word for word in source if word["line"] != "subtitle"],
                [dict(word, top=62, line="workspace") if word["line"] in ("subtitle", "customize") else word for word in source]):
            with self.subTest(capture=capture), self.assertRaisesRegex(RuntimeError, "dashboard body"):
                self.fresh_pass(source, capture)

    def test_competing_body_controls_are_not_hidden_by_another_ocr_mode(self):
        source = words()
        duplicate = source + [dict(word, top=220, line="duplicate") for word in source if word["line"] == "customize"]
        with self.assertRaises(settings.ocr.AmbiguousLabel):
            self.fresh_pass(source, {11: duplicate, 3: source, 6: source})

    def test_body_markers_cannot_be_combined_across_ocr_layouts(self):
        source = words()
        customize_only = [word for word in source if word["line"] != "subtitle"]
        subtitle_only = [word for word in source if word["line"] != "customize"]
        with self.assertRaisesRegex(RuntimeError, "dashboard body"):
            self.fresh_pass(source, {11: customize_only, 3: subtitle_only, 6: customize_only})


if __name__ == "__main__":
    unittest.main()
