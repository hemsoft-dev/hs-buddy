"""Replay real installed loading/painted captures through the lifecycle caller."""
import contextlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import types
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("crew_lifecycle", HERE / "linux-installed-lifecycle.py")
lifecycle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lifecycle)
github = lifecycle.load(HERE / "github-desktop.py")


class ReachedBrowser(Exception):
    """Stop after the real Crew caller, before any browser interaction."""


class CrewCaptureControls(unittest.TestCase):
    def exercise(self, root, frames, *, checker=None, visible=True, capture_error=None):
        profile = root / "profile"
        profile.mkdir()
        (root / "electron-seed").mkdir()
        lifecycle.write(profile / "config.json", lifecycle.config_fixture())
        value = dict(projects=[dict(id=lifecycle.PROJECT)],
            session=dict(status="error", conversationHistory=[dict(content=lifecycle.MESSAGE)]),
            cache=dict(data=dict(fixture=lifecycle.MARKER)), viewMode="list")
        def evaluate(expression):
            if "config:get-store-path" in expression:
                return str(profile / "config.json")
            if expression.startswith("(async()=>({"):
                return value
            if "querySelectorAll('h2')" in expression:
                return visible() if callable(visible) else visible
            return True
        cdp = Mock(evaluate=evaluate)
        process = Mock(pid=123, is_running=lambda: True)
        captures = []
        def screenshot(path):
            if capture_error:
                raise capture_error
            path.parent.mkdir(parents=True, exist_ok=True)
            source = frames[min(len(captures), len(frames) - 1)]
            path.write_bytes(source.read_bytes())
            captures.append(path)
        window = Mock(window="synthetic-window", screenshot=screenshot)
        def wait_for(read, process, timeout=60):
            for _ in range(3):
                result = read()
                if result:
                    return result
            raise TimeoutError("Controlled readiness deadline")
        paired = types.SimpleNamespace(CDP=lambda *_: cdp, wait_for=wait_for)
        original = subprocess.check_output
        def check_output(args, **kwargs):
            if args[:2] == ["xdotool", "getwindowgeometry"]:
                return "WIDTH=1200\nHEIGHT=820\n"
            return original(args, **kwargs)
        check = checker or github.check
        with patch.object(lifecycle, "load", side_effect=lambda path: paired if path.name == "native-comparison.py" else github), \
                patch.object(lifecycle, "running", return_value=contextlib.nullcontext((process, window))), \
                patch.object(lifecycle, "recorded", return_value=contextlib.nullcontext()), \
                patch.object(lifecycle.subprocess, "check_output", side_effect=check_output), \
                patch("psutil.Process", return_value=process), \
                patch.object(github, "check", side_effect=check), \
                patch.object(lifecycle, "open_browser_tab", side_effect=ReachedBrowser):
            try:
                lifecycle.electron_pass(Path("synthetic-executable"), profile,
                    types.SimpleNamespace(url="http://127.0.0.1:9"), root, "electron-seed",
                    dict(proxy="http://127.0.0.1:9", url="http://203.0.113.1:9"))
            finally:
                self.captures = captures

    @unittest.skipUnless(shutil.which("tesseract"), "Captured Linux OCR replay needs Tesseract")
    def test_actual_caller_rejects_loading_then_accepts_painted_owned_capture(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaises(ReachedBrowser):
                self.exercise(root, [HERE / "fixtures/linux-lifecycle-crew-loading.png",
                    HERE / "fixtures/linux-lifecycle-crew-painted.png"])
            self.assertEqual(len(self.captures), 2)
            self.assertEqual((root / "electron-seed/crew-restored.png").read_bytes(), self.captures[1].read_bytes())
            attempts = lifecycle.read(root / "electron-seed/crew-capture-attempts.json")
            self.assertEqual([x["result"] for x in attempts], ["missing-visible-text", "accepted"])

    def test_permanent_missing_content_times_out_without_a_success_capture(self):
        def missing(*_):
            raise RuntimeError("Required markers absent") from github.settings.MissingVisibleText("Missing conversation")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaisesRegex(TimeoutError, "Controlled readiness deadline"):
                self.exercise(root, [HERE / "fixtures/linux-lifecycle-crew-loading.png"], checker=missing)
            self.assertEqual(len(self.captures), 3)
            self.assertFalse((root / "electron-seed/crew-restored.png").exists())
            attempts = lifecycle.read(root / "electron-seed/crew-capture-attempts.json")
            self.assertTrue(all(x["result"] == "missing-visible-text" for x in attempts))

    def test_ocr_engine_error_is_not_retried_as_missing_content(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaisesRegex(RuntimeError, "Controlled OCR engine failure"):
                self.exercise(root, [HERE / "fixtures/linux-lifecycle-crew-loading.png"],
                    checker=Mock(side_effect=RuntimeError("Controlled OCR engine failure")))
            self.assertEqual(len(self.captures), 1)
            self.assertFalse((root / "electron-seed/crew-restored.png").exists())

    def test_dom_missing_content_cannot_be_replaced_by_a_good_image(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaises(TimeoutError):
                self.exercise(root, [HERE / "fixtures/linux-lifecycle-crew-painted.png"], visible=False)
            self.assertEqual(self.captures, [])
            self.assertFalse((root / "electron-seed/crew-restored.png").exists())

    def test_dom_change_after_a_painted_frame_prevents_acceptance(self):
        observations = iter([True, False] * 3)
        def checked(image, *_):
            image.with_suffix(".ocr.txt").write_text("Synthetic control: painted markers")
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaises(TimeoutError):
                self.exercise(root, [HERE / "fixtures/linux-lifecycle-crew-painted.png"],
                    checker=checked, visible=lambda: next(observations))
            self.assertEqual(len(self.captures), 3)
            self.assertFalse((root / "electron-seed/crew-restored.png").exists())
            attempts = lifecycle.read(root / "electron-seed/crew-capture-attempts.json")
            self.assertTrue(all(x["result"] == "dom-changed" for x in attempts))

    def test_capture_io_error_fails_without_retrying(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaisesRegex(RuntimeError, "capture or evidence IO failed"):
                self.exercise(root, [HERE / "fixtures/linux-lifecycle-crew-painted.png"],
                    capture_error=OSError("Controlled capture IO failure"))
            attempts = lifecycle.read(root / "electron-seed/crew-capture-attempts.json")
            self.assertEqual(len(attempts), 1)
            self.assertEqual(attempts[0]["result"], "capture-error")
