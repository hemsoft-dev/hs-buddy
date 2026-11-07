"""Browser fixture contrast and painted-capture controls, not installed proof."""
from html.parser import HTMLParser
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import Mock
import urllib.request

from test_linux_lifecycle_crew_capture import lifecycle, github


class BodyStyle(HTMLParser):
    def handle_starttag(self, tag, attrs):
        if tag == "body":
            self.style = dict(piece.strip().split(":", 1) for piece in dict(attrs)["style"].split(";") if piece.strip())


class BrowserCaptureControls(unittest.TestCase):
    def test_actual_http_fixture_has_explicit_high_contrast_body(self):
        with lifecycle.browser_fixture() as fixture:
            with urllib.request.urlopen(fixture["proxy"] + "/?seed=1", timeout=5) as response:
                html = response.read().decode()
        body = BodyStyle()
        body.feed(html)
        # Black on opaque white is 21:1; the old transparent body inherited
        # the installed dark canvas and failed the mixed-window OCR oracle.
        self.assertEqual(body.style.get("color"), "#000")
        self.assertEqual(body.style.get("background"), "#fff")
        self.assertIn("localStorage.setItem('lifecycle'", html)
        self.assertIn("document.cookie='lifecycle=", html)
        self.assertIn(lifecycle.MARKER, html)

    def exercise(self, root, *, outcomes=(False, True), state=None, engine_error=None, io_error=None):
        state = state or dict(marker=lifecycle.MARKER, cookie="lifecycle=" + lifecycle.MARKER,
            text="Lifecycle browser " + lifecycle.MARKER, readyState="complete", visibility="visible",
            host=dict(bounds=dict(width=846, height=696)), viewport=dict(width=846, height=696))
        cdp = Mock(evaluate=lambda *_: state)
        calls = []
        def screenshot(path):
            if io_error:
                raise OSError("Controlled capture IO")
            path.write_bytes(b"synthetic-owned-window")
        def check(image, *markers, **kwargs):
            self.assertEqual(markers, ("Lifecycle browser", lifecycle.MARKER))
            self.assertTrue(kwargs["browser"])
            calls.append(image)
            if engine_error:
                raise RuntimeError("Controlled engine failure")
            if not outcomes[min(len(calls)-1, len(outcomes)-1)]:
                raise RuntimeError("Missing required browser content") from github.settings.MissingVisibleText("Missing marker")
            image.with_suffix(".ocr.txt").write_text("Synthetic control: both markers")
        def wait_for(read, process, timeout=60):
            self.assertEqual(timeout, 60)
            for _ in range(3):
                if read():
                    return
            raise TimeoutError("Controlled deadline")
        paired = types.SimpleNamespace(wait_for=wait_for)
        lifecycle.capture_browser(cdp, types.SimpleNamespace(screenshot=screenshot), paired, Mock(), root,
            types.SimpleNamespace(check=check, settings=github.settings))
        return calls

    def test_paint_wait_retries_only_missing_text_and_retains_raw_attempts(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            calls = self.exercise(root)
            self.assertEqual(len(calls), 2)
            self.assertEqual((root / "browser-restored.png").read_bytes(), calls[1].read_bytes())
            self.assertEqual([x["result"] for x in lifecycle.read(root / "browser-capture-attempts.json")],
                ["missing-visible-text", "accepted"])
            self.assertEqual(lifecycle.read(root / "browser-state.json")["marker"], lifecycle.MARKER)

    def test_permanent_blank_exhausts_existing_deadline_without_success_image(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaises(TimeoutError):
                self.exercise(root, outcomes=(False,))
            self.assertFalse((root / "browser-restored.png").exists())
            self.assertEqual(len(lifecycle.read(root / "browser-capture-attempts.json")), 3)

    def test_engine_and_io_failures_are_not_retried(self):
        for kwargs, message in [(dict(engine_error=True), "engine failure"), (dict(io_error=True), "capture or evidence IO")]:
            with self.subTest(kwargs=kwargs), tempfile.TemporaryDirectory() as raw:
                root = Path(raw)
                with self.assertRaisesRegex(RuntimeError, message):
                    self.exercise(root, **kwargs)
                self.assertEqual(len(lifecycle.read(root / "browser-capture-attempts.json")), 1)

    def test_storage_loss_fails_before_capture_and_keeps_readback(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            with self.assertRaisesRegex(RuntimeError, "localStorage/cookie"):
                self.exercise(root, state=dict(marker="lost", cookie=""))
            self.assertEqual(lifecycle.read(root / "browser-state.json")["marker"], "lost")
            self.assertFalse((root / "browser-restored.png").exists())
