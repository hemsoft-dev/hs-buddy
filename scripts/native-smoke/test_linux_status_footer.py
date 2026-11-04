"""Actual browser check on the retained hosted compact capture.

Only the engine boundary is recorded: real footer measurement, TSV parsing,
content crop and marker checks run. Preprocessing/consumer proof, not installed
or native qualification.
"""
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("linux_status_footer_check", HERE / "github-desktop.py")
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)
settings, ocr = desktop.settings, desktop.settings.ocr
FIXTURES = HERE / "fixtures"
CAPTURE = FIXTURES / "ubuntu-4d6-compact-browser.png"
PROVENANCE = json.loads((FIXTURES / "ubuntu-4d6-compact-browser.json").read_text(encoding="utf-8"))
LAYOUTS = json.loads((FIXTURES / "ubuntu-4d6-compact-layouts.json").read_text(encoding="utf-8"))
MARKERS = ("FIXTURE PAGE", "Buddy Fixture OK")
NO_FOOTER = "no unique native status footer"
DEFAULT_FILTERS = ["scale=iw*3:ih*3:flags=neighbor", "scale=iw*3:ih*3:flags=neighbor,format=gray,negate"]
SMOOTH_FILTERS = ["scale=iw*3:ih*3:flags=neighbor", "scale=iw*2:ih*2:flags=lanczos,format=gray,negate"]
SMOOTH = ".ocr-scale2-inverted"


def rows_of(tsv):
    lines = tsv.splitlines()
    return lines[0], [line.split("\t") for line in lines[1:]]


def tsv_of(header, rows):
    return "\n".join([header] + ["\t".join(row) for row in rows]) + "\n"


def without(tsv, *labels):
    header, rows = rows_of(tsv)
    return tsv_of(header, [row for row in rows if not (row[0] == "5" and ocr.normalize(row[11]).endswith(labels))])


def respell(tsv, label, replacement):
    header, rows = rows_of(tsv)
    for row in rows:
        if row[0] == "5" and ocr.normalize(row[11]).endswith(label):
            row[11] = replacement
    return tsv_of(header, rows)


def footer_rows(tsv):
    return [row for row in rows_of(tsv)[1] if row[0] == "5" and ocr.normalize(row[11]).endswith(("prs", "schedules", "jobs"))]


def recorded_png(width, height):
    return b"\x89PNG\r\n\x1a\n" + b"\0" * 8 + struct.pack(">II", width, height)


class Replay:
    """Serve recorded target-engine output at the two process boundaries."""

    def __init__(self, directory, footer=None, content=None):
        self.image = Path(directory) / "capture.png"
        self.image.write_bytes(CAPTURE.read_bytes())
        self.footer = footer or (lambda _suffix, _mode, tsv: tsv)
        self.content = content or (lambda _suffix, _mode, text: text)
        self.filters, self.footer_calls, self.content_calls = [], [], []

    def run(self, *args, **_kwargs):
        if args[0] == "ffmpeg":
            source = Path(args[args.index("-i") + 1])
            width, height = struct.unpack(">II", source.read_bytes()[16:24])
            chain = args[args.index("-vf") + 1]
            self.filters.append(chain)
            crop = re.match(r"crop=iw:(\d+):0:0:exact=1$", chain)
            scale = re.match(r"scale=iw\*(\d):ih\*\1:", chain)
            if crop:
                height = int(crop.group(1))
            else:
                width, height = width * int(scale.group(1)), height * int(scale.group(1))
            Path(args[-1]).write_bytes(recorded_png(width, height))
            return ""
        name, mode = Path(args[1]).name, str(args[args.index("--psm") + 1])
        suffix = name.replace("capture.browser-ocr", "").replace(".png", "")
        self.content_calls.append((suffix, mode))
        return self.content(suffix, mode, LAYOUTS["contentText"][suffix][mode])

    def engine(self, command, **_kwargs):
        name, mode = Path(command[1]).name, command[command.index("--psm") + 1]
        suffix = name.replace("capture", "").replace(".png", "")
        self.footer_calls.append((suffix, mode))
        return SimpleNamespace(stdout=self.footer(suffix, mode, LAYOUTS["footerTsv"][suffix][mode]))

    def __enter__(self):
        self.patches = [patch.object(settings, "run", side_effect=self.run),
            patch.object(ocr.subprocess, "run", side_effect=self.engine),
            patch.object(ocr.shutil, "which", return_value="tesseract")]
        for active in self.patches:
            active.start()
        return self

    def __exit__(self, *_details):
        for active in self.patches:
            active.stop()

    @property
    def crops(self):
        return [chain for chain in self.filters if chain.startswith("crop=")]


class RetainedCompactCapture(unittest.TestCase):
    def setUp(self):
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.directory = directory.name

    def check(self, replay, platform="linux", **kwargs):
        with patch.object(desktop.sys, "platform", platform):
            return desktop.check(replay.image, *MARKERS, browser=True, **kwargs)

    def test_fixture_is_the_retained_original_capture(self):
        self.assertEqual(hashlib.sha256(CAPTURE.read_bytes()).hexdigest(), PROVENANCE["sha256"])
        self.assertEqual(struct.unpack(">II", CAPTURE.read_bytes()[16:24]), (900, 650))
        self.assertEqual(PROVENANCE["engine"]["tesseract"], "tesseract 5.3.4")

    def test_default_profile_still_rejects_the_capture_in_all_nine_layouts(self):
        for platform in ("win32", "darwin"):
            with self.subTest(platform=platform), Replay(self.directory) as replay:
                with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
                    self.check(replay, platform)
                self.assertEqual(replay.filters, DEFAULT_FILTERS)
                self.assertEqual(len(replay.footer_calls), 9)
                self.assertEqual(replay.crops, [])

    def test_direct_default_measurement_is_unchanged_on_linux(self):
        with Replay(self.directory) as replay, patch.object(desktop.sys, "platform", "linux"):
            with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
                ocr.status_content_image(replay.image, settings.run)
            self.assertEqual(replay.filters, DEFAULT_FILTERS)

    def test_linux_browser_check_measures_one_layout_and_crops_exactly_the_footer(self):
        with Replay(self.directory) as replay:
            original = replay.image.read_bytes()
            self.check(replay)
            self.assertEqual(replay.image.read_bytes(), original)
            self.assertEqual(replay.footer_calls, [(suffix, mode)
                for suffix in ("", ".ocr-scale3") for mode in ("11", "3", "6")] + [(SMOOTH, "11")])
            self.assertEqual(replay.filters, SMOOTH_FILTERS + ["crop=iw:628:0:0:exact=1"] + SMOOTH_FILTERS)
            self.assertEqual(replay.content_calls, [(suffix, mode)
                for suffix in ("", ".ocr-scale3") for mode in ("11", "3", "6")] + [(SMOOTH, "11")])
            accepted = replay.image.with_suffix(".ocr.txt").read_text(encoding="utf-8")
            self.assertIn("fixture page", accepted)
            for footer_word in ("schedules", "jobs", "prs"):
                self.assertNotIn(footer_word, accepted)
            retained = json.loads(replay.image.with_name("capture.ocr-scale2-inverted.footer-ocr-11.json").read_text(encoding="utf-8"))
            self.assertEqual([word["text"] for word in retained][-6:], ["0", "PRs", "0", "schedules", "0", "jobs"])

    def test_a_missing_footer_fails_without_a_crop(self):
        with Replay(self.directory, footer=lambda _s, _m, tsv: without(tsv, "prs", "schedules", "jobs")) as replay:
            with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
                self.check(replay)
            self.assertEqual(len(replay.footer_calls), 9)
            self.assertEqual(replay.crops, [])

    def test_labels_from_different_layouts_are_not_united(self):
        missing = {"11": "jobs", "3": "prs", "6": "schedules"}
        def footer(suffix, mode, tsv):
            return without(tsv, missing[mode]) if suffix == SMOOTH else tsv
        with Replay(self.directory, footer=footer) as replay:
            with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
                self.check(replay)
            self.assertEqual(replay.crops, [])

    def test_a_q_misread_of_the_zero_stays_refused(self):
        def footer(suffix, _mode, tsv):
            return respell(tsv, "jobs", "OQjobs") if suffix == SMOOTH else tsv
        with Replay(self.directory, footer=footer) as replay:
            with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
                self.check(replay)
            self.assertEqual(replay.crops, [])

    def test_a_duplicate_label_in_the_row_is_not_a_footer(self):
        def footer(suffix, _mode, tsv):
            if suffix != SMOOTH:
                return tsv
            header, rows = rows_of(tsv)
            extra = [row[:] for row in footer_rows(tsv) if ocr.normalize(row[11]).endswith("schedules")]
            for row in extra:
                row[6] = str(int(row[6]) + 300)
            return tsv_of(header, rows + extra)
        with Replay(self.directory, footer=footer) as replay:
            with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
                self.check(replay)
            self.assertEqual(replay.crops, [])

    def test_a_competing_footer_cannot_be_retried_away(self):
        def footer(suffix, _mode, tsv):
            if suffix != SMOOTH:
                return tsv
            header, rows = rows_of(tsv)
            extra = [row[:] for row in footer_rows(tsv)]
            for row in extra:
                row[7] = str(int(row[7]) - 20)
            return tsv_of(header, rows + extra)
        with Replay(self.directory, footer=footer) as replay:
            with self.assertRaises(ocr.AmbiguousLabel):
                self.check(replay)
            self.assertEqual(replay.footer_calls[-1], (SMOOTH, "11"))
            self.assertEqual(len(replay.footer_calls), 7)
            self.assertEqual(replay.crops, [])

    def test_malformed_engine_output_fails(self):
        def edited(change):
            def footer(suffix, _mode, tsv):
                if suffix != SMOOTH:
                    return tsv
                header, rows = rows_of(tsv)
                return tsv_of(header, [change(row) if row[0] == "5" else row for row in rows])
            return footer
        for name, broken, expected in (("headerless", lambda *_: "garbage", RuntimeError),
                ("empty", lambda *_: "", RuntimeError),
                ("non-numeric", edited(lambda row: row[:6] + ["left"] + row[7:]), ValueError),
                ("truncated", edited(lambda row: row[:11]), AttributeError)):
            with self.subTest(name), Replay(self.directory, footer=broken) as replay:
                with self.assertRaises(expected):
                    self.check(replay)
                self.assertEqual(replay.crops, [])

    def test_a_footer_at_the_window_top_leaves_invalid_content_bounds(self):
        def footer(suffix, _mode, tsv):
            if suffix != SMOOTH:
                return tsv
            header, _ = rows_of(tsv)
            rows = footer_rows(tsv)
            for row in rows:
                row[7] = "0"
            return tsv_of(header, rows)
        with Replay(self.directory, footer=footer) as replay:
            with self.assertRaisesRegex(RuntimeError, "invalid content bounds"):
                self.check(replay)
            self.assertEqual(replay.crops, [])

    def test_each_missing_body_marker_fails_after_the_bounded_layouts(self):
        for marker in MARKERS:
            def content(_suffix, _mode, text, marker=marker):
                return re.sub(re.escape(marker), "", " ".join(text.split()), flags=re.IGNORECASE)
            with self.subTest(marker=marker), Replay(self.directory, content=content) as replay:
                with self.assertRaisesRegex(RuntimeError, "one OCR layout") as caught:
                    self.check(replay)
                self.assertIsInstance(caught.exception.__cause__, settings.MissingVisibleText)
                self.assertEqual(len(replay.content_calls), 9)
                self.assertEqual(replay.crops, ["crop=iw:628:0:0:exact=1"])

    def test_footer_text_cannot_supply_a_body_marker(self):
        # The measured footer layout does read this word; only the crop counts.
        with Replay(self.directory) as replay:
            with patch.object(desktop.sys, "platform", "linux"), self.assertRaisesRegex(RuntimeError, "one OCR layout"):
                desktop.check(replay.image, "schedules", browser=True)
            self.assertEqual(len(replay.content_calls), 9)

    def test_an_absent_marker_in_any_attempted_layout_fails(self):
        def content(suffix, _mode, text):
            return text + "\nOther private account" if suffix == SMOOTH else text
        with Replay(self.directory, content=content) as replay:
            with self.assertRaisesRegex(RuntimeError, "Another account"):
                self.check(replay, absent=("Other private account",))
            self.assertEqual(replay.content_calls[-1], (SMOOTH, "11"))

    def test_engine_and_io_errors_are_not_retried_away(self):
        failure = subprocess.CalledProcessError(1, "tesseract")
        def footer_engine(suffix, _mode, _tsv):
            raise failure
        with Replay(self.directory, footer=footer_engine) as replay:
            with self.assertRaises(subprocess.CalledProcessError):
                self.check(replay)
            self.assertEqual(len(replay.footer_calls), 1)
        def body_engine(_suffix, _mode, _text):
            raise failure
        with Replay(self.directory, content=body_engine) as replay:
            with self.assertRaises(subprocess.CalledProcessError):
                self.check(replay)
            self.assertEqual(len(replay.content_calls), 1)
        def crop_io(*args, **kwargs):
            if args[0] == "ffmpeg" and any(str(arg).startswith("crop=") for arg in args):
                raise OSError("Controlled crop IO")
            return replay.run(*args, **kwargs)
        with Replay(self.directory) as replay, patch.object(settings, "run", side_effect=crop_io):
            with self.assertRaisesRegex(OSError, "Controlled crop IO"):
                self.check(replay)


def target_engine():
    """Only the hosted job's engine may judge the recorded layouts."""
    if sys.platform != "linux" or not shutil.which("tesseract") or not shutil.which("ffmpeg"):
        return False
    version = subprocess.run(["tesseract", "--version"], capture_output=True, text=True, timeout=10).stdout
    return version.splitlines()[:1] == [PROVENANCE["engine"]["tesseract"]]


@unittest.skipUnless(target_engine(), "Needs the hosted job's Tesseract 5.3.4 and ffmpeg")
class TargetEngineReplay(unittest.TestCase):
    """The same capture and consumer with nothing recorded."""

    def setUp(self):
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.image = Path(directory.name) / "capture.png"
        self.image.write_bytes(CAPTURE.read_bytes())

    def test_linux_browser_reads_the_retained_capture_with_the_target_engine(self):
        # Historical old-red evidence is retained separately. Other CPUs may
        # improve its OCR; this regression requires the corrected consumer.
        desktop.check(self.image, *MARKERS, browser=True)
        self.assertEqual(self.image.read_bytes(), CAPTURE.read_bytes())
        accepted = self.image.with_suffix(".ocr.txt").read_text(encoding="utf-8")
        self.assertIn("buddy fixture ok", accepted)
        self.assertNotIn("schedules", accepted)

    def test_a_capture_without_the_footer_is_refused(self):
        settings.run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", CAPTURE,
            "-vf", "crop=iw:628:0:0:exact=1", "-frames:v", "1", self.image)
        with self.assertRaisesRegex(RuntimeError, NO_FOOTER):
            desktop.check(self.image, *MARKERS, browser=True)


if __name__ == "__main__":
    unittest.main()
