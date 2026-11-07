"""Actual check consumer with recorded, individually retained engine layouts."""
import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

HERE = Path(__file__).parent
spec = importlib.util.spec_from_file_location("browser_check", HERE / "github-desktop.py")
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)
MARKERS = ("PROBE PAGE", "Downloads are blocked", "Bookmarks", "Buddy Probe")

class CapturedLinuxAnalysis(unittest.TestCase):
    def run_check(self, *, new=True, missing=None, forbidden=False):
        data = json.loads((HERE / "fixtures/ubuntu-73d-browser-layouts.json").read_text())
        filters = []
        def command(*args, **kwargs):
            if args[0] == "ffmpeg":
                filters.append(args[args.index("-vf") + 1])
                return ""
            mode = str(args[args.index("--psm") + 1])
            name = str(args[1])
            if ".ocr-scale2-inverted" in name:
                text = data["controlledNew"][mode]
            else:
                suffix = ".ocr-scale3-inverted" if "inverted" in name else ".ocr-scale3" if "scale3" in name else ""
                text = data["hostedOld"][suffix + ":" + mode]
            if missing:
                import re
                text = re.sub(re.escape(missing), "", text, flags=re.IGNORECASE)
            if forbidden and ".ocr-scale2-inverted" in name:
                text += "\nOther private account"
            return text
        with TemporaryDirectory() as temporary:
            image = Path(temporary) / "capture.png"
            image.write_bytes((HERE / "fixtures/ubuntu-73d-browser-content.png").read_bytes())
            original = image.read_bytes()
            with patch.object(desktop.sys, "platform", "linux" if new else "win32"), \
                    patch.object(desktop.settings.ocr, "status_content_image", return_value=image), \
                    patch.object(desktop.settings, "run", side_effect=command):
                desktop.check(image, *MARKERS, browser=True, absent=("Other private account",))
            self.assertEqual(image.read_bytes(), original)
            self.assertEqual(len(filters), 2)  # Same 3 candidates, no additional attempts.
            self.assertIn("scale=iw*2:ih*2:flags=lanczos,format=gray,negate", filters)

    def test_footer_and_body_use_one_profile_selected_only_on_linux(self):
        for platform, smooth in (("linux", True), ("win32", False), ("darwin", False)):
            image = Path("capture.png")
            with self.subTest(platform=platform), patch.object(desktop.sys, "platform", platform), \
                    patch.object(desktop.settings.ocr, "status_content_image", return_value=image) as status, \
                    patch.object(desktop.settings.ocr, "analysis_images", return_value=iter([image])) as analysis:
                self.assertEqual(list(desktop.browser_analysis(image)), [image])
                self.assertEqual(status.call_args.kwargs, dict(suffix=".browser-ocr.png", smooth_inverted=smooth))
                self.assertEqual(analysis.call_args.kwargs, dict(smooth_inverted=smooth))

    def test_hosted_old_layouts_fail_without_union_and_controlled_candidate_passes(self):
        with self.assertRaisesRegex(RuntimeError, "one OCR layout"):
            self.run_check(new=False)
        self.run_check()

    def test_each_missing_marker_still_fails(self):
        for marker in MARKERS:
            with self.subTest(marker=marker), self.assertRaisesRegex(RuntimeError, "one OCR layout"):
                self.run_check(missing=marker)

    def test_new_candidate_cannot_hide_other_account_data(self):
        with self.assertRaisesRegex(RuntimeError, "Another account"):
            self.run_check(forbidden=True)
