"""Keep exact footer OCR evidence without qualifying missing/ambiguous chrome."""
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("footer_retention_ocr", Path(__file__).with_name("settings-ocr.py"))
OCR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OCR)


def labels(top=100, height=10):
    return [dict(text=text, left=left, top=top, width=50, height=height, line="fixture")
            for text, left in [("PRs", 20), ("schedules", 100), ("jobs", 220)]]


class FooterAnalysisRetentionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.images = [self.root / name for name in ["capture.png", "capture.ocr-scale3.png", "capture.ocr-scale3-inverted.png"]]
        for image in self.images:
            image.write_bytes(b"\x89PNG\r\n\x1a\n" + b"\0" * 8 + struct.pack(">II", 300, 200))

    def invoke(self, words, images=None):
        engine = mock.Mock(return_value=words)
        runner = mock.Mock()
        analysis = mock.patch.object(OCR, "analysis_images", return_value=iter(images or self.images))
        return analysis, mock.patch.object(OCR, "recognize", engine), engine, runner

    def test_missing_footer_retains_all_actual_layouts_and_still_fails(self):
        words = labels()[:2]
        analysis, recognize, engine, runner = self.invoke(words)
        with analysis, recognize:
            with self.assertRaisesRegex(RuntimeError, "no unique native status footer"):
                OCR.status_content_image(self.images[0], runner)
        self.assertEqual(engine.call_args_list, [mock.call(image, mode=mode) for image in self.images for mode in (11, 3, 6)])
        runner.assert_not_called()
        for image in self.images:
            for mode in (11, 3, 6):
                self.assertEqual(json.loads(image.with_suffix(f".footer-ocr-{mode}.json").read_text()), words)

    def test_ambiguous_footer_retains_first_layout_without_later_mode(self):
        words = labels(90, 20) + labels(110, 20)
        analysis, recognize, engine, runner = self.invoke(words)
        with analysis, recognize:
            with self.assertRaises(OCR.AmbiguousLabel):
                OCR.status_content_image(self.images[0], runner)
        engine.assert_called_once_with(self.images[0], mode=11)
        runner.assert_not_called()
        self.assertEqual(json.loads(self.images[0].with_suffix(".footer-ocr-11.json").read_text()), words)

    def test_valid_footer_keeps_original_crop_and_one_recognition(self):
        words = labels()
        analysis, recognize, engine, runner = self.invoke(words)
        with analysis, recognize:
            result = OCR.status_content_image(self.images[0], runner)
        engine.assert_called_once_with(self.images[0], mode=11)
        self.assertEqual(result, self.images[0].with_suffix(".content-ocr.png"))
        self.assertIn("crop=iw:95:0:0:exact=1", runner.call_args.args)
        self.assertEqual(json.loads(self.images[0].with_suffix(".footer-ocr-11.json").read_text()), words)

    def test_receipt_write_failure_cannot_turn_into_qualification(self):
        analysis, recognize, engine, runner = self.invoke(labels())
        failure = OSError("controlled receipt failure")
        with analysis, recognize, mock.patch.object(Path, "write_text", side_effect=failure):
            with self.assertRaises(OSError) as caught:
                OCR.status_content_image(self.images[0], runner)
        self.assertIs(caught.exception, failure)
        engine.assert_called_once_with(self.images[0], mode=11)
        runner.assert_not_called()


if __name__ == "__main__":
    unittest.main()
