"""Replay Intel's border-absorbing footer OCR without weakening label checks."""
import base64
import importlib.util
import json
from pathlib import Path
import re
import struct
import tempfile
import unittest
import zlib

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('footer_ocr', HERE / 'settings-ocr.py')
ocr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ocr)


class FooterBoundsTests(unittest.TestCase):
    def fixture(self):
        return json.loads((HERE / 'fixtures/footer-intel-border-box.json').read_text())

    def capture(self, root, fixture, backing=1):
        image = Path(root) / 'locator.png'
        size = [fixture['width'] * backing, fixture['height'] * backing]
        image.write_bytes(b'\x89PNG\r\n\x1a\n' + bytes(8) + struct.pack('>II', *size))
        receipt = dict(fixture['metadata'], capturePixelSize=size, capturePixelsPerWindowPoint=backing)
        image.with_suffix('.window.json').write_text(json.dumps(receipt))
        return image

    def test_recorded_intel_box_ends_the_search_above_the_real_separator(self):
        fixture = self.fixture()
        self.assertEqual(ocr.status_footer(fixture['footerWords']), fixture['footer'])
        with tempfile.TemporaryDirectory() as root:
            image = self.capture(root, fixture)
            def run(*args):
                Path(args[-1]).write_bytes(zlib.decompress(base64.b64decode(fixture['pixels'])))
            self.assertEqual(ocr.footer_border(image, fixture['footer'], 1, run), fixture['expectedContentHeight'])

    def test_known_height_follows_the_rendered_shell_contract(self):
        source = (HERE.parent.parent / 'rust/crates/buddy-app/src/shell/status_bar.rs').read_text()
        height = float(re.search(r'STATUS_BAR_HEIGHT: f32 = ([0-9.]+)', source)[1])
        self.assertEqual(ocr.STATUS_BAR_HEIGHT, height)

    def test_backing_scale_and_shell_zoom_scale_the_fallback(self):
        fixture = self.fixture()
        with tempfile.TemporaryDirectory() as root:
            for backing, zoom in [(1, 1), (3, 1), (2, 1.5)]:
                image = self.capture(root, fixture, backing)
                footer = {key: value * backing for key, value in fixture['footer'].items()}
                expected = round(fixture['height'] * backing - 22 * backing * zoom)
                self.assertEqual(ocr.known_footer_top(image, footer, 1, zoom), expected)

    def test_invalid_receipts_and_nonfooter_text_cannot_qualify_the_fallback(self):
        fixture = self.fixture()
        with tempfile.TemporaryDirectory() as root:
            image = self.capture(root, fixture)
            receipt = fixture['metadata']
            for broken in [dict(receipt, capturePixelSize=[1, 1]),
                           dict(receipt, capturePixelsPerWindowPoint=0),
                           dict(receipt, capturePixelsPerWindowPoint=float('nan'))]:
                image.with_suffix('.window.json').write_text(json.dumps(broken))
                with self.assertRaises(RuntimeError):
                    ocr.known_footer_top(image, fixture['footer'], 1)
            self.capture(root, fixture)
            with self.assertRaises(RuntimeError):
                ocr.known_footer_top(image, dict(fixture['footer'], top=500), 1)
            for zoom in (0, float('nan'), float('inf'), 1e308):
                with self.assertRaises(RuntimeError):
                    ocr.known_footer_top(image, fixture['footer'], 1, zoom)

    def test_no_separator_and_no_receipt_still_fails_closed(self):
        fixture = self.fixture()
        with tempfile.TemporaryDirectory() as root:
            image = self.capture(root, fixture)
            image.with_suffix('.window.json').unlink()
            def run(*args):
                Path(args[-1]).write_bytes(zlib.decompress(base64.b64decode(fixture['pixels'])))
            with self.assertRaisesRegex(RuntimeError, 'no visible separator'):
                ocr.footer_border(image, fixture['footer'], 1, run)
