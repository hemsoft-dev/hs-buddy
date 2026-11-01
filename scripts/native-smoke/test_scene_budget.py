import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


runner = load("scene-runner")
evidence = load("prepare-evidence")


class EvidenceStaging(unittest.TestCase):
    def test_preserves_receipts_and_does_not_follow_fixture_links(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source = root / "evidence"
            source.mkdir()
            (source / "actions.jsonl").write_text('{"action":"outside-dark"}\n')
            (source / "frame.png").write_bytes(b"capture")
            outside = root / "outside"
            outside.mkdir()
            (outside / "keep.txt").write_text("outside")
            try:
                (source / "outside-link").symlink_to(outside, target_is_directory=True)
                (source / "broken-link").symlink_to(root / "gone")
            except OSError as failure:
                self.skipTest(f"Symlink creation unavailable: {failure}")
            receipt = evidence.stage(source, root / "upload")
            self.assertEqual(receipt["files"], 2)
            self.assertEqual(receipt["skippedLinksOrSpecialFiles"], ["broken-link", "outside-link"])
            self.assertEqual((root / "upload/actions.jsonl").read_bytes(), (source / "actions.jsonl").read_bytes())
            self.assertEqual((root / "upload/frame.png").read_bytes(), b"capture")
            self.assertFalse((root / "upload/outside-link").exists())
            self.assertEqual((outside / "keep.txt").read_text(), "outside")

    def test_rejects_recursive_staging(self):
        with tempfile.TemporaryDirectory() as raw:
            with self.assertRaises(ValueError):
                evidence.stage(Path(raw), Path(raw) / "upload")

    @unittest.skipUnless(os.name == "nt", "Windows Crew junction fixture")
    def test_does_not_follow_windows_junctions(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source, outside = root / "evidence", root / "outside"
            source.mkdir()
            outside.mkdir()
            (outside / "keep.txt").write_text("outside")
            junction = source / "outside-link"
            subprocess.run(["cmd.exe", "/d", "/c", "mklink", "/J", str(junction), str(outside)],
                check=True, capture_output=True, creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                receipt = evidence.stage(source, root / "upload")
                self.assertEqual(receipt["files"], 0)
                self.assertEqual(receipt["skippedLinksOrSpecialFiles"], ["outside-link"])
                self.assertEqual((outside / "keep.txt").read_text(), "outside")
            finally:
                junction.rmdir()


@unittest.skipUnless(os.name == "posix", "Unix scene process groups")
class SceneBudget(unittest.TestCase):
    def test_failure_remains_failure_with_timing_receipts(self):
        with tempfile.TemporaryDirectory() as raw:
            receipt = Path(raw) / "timing.jsonl"
            code = runner.run_scene("settings", [sys.executable, "-c", "raise SystemExit(7)"], 5, receipt)
            self.assertEqual(code, 7)
            records = [json.loads(line) for line in receipt.read_text().splitlines()]
            self.assertEqual([r["state"] for r in records], ["started", "failed"])
            self.assertEqual(records[-1]["exitCode"], 7)

    def test_timeout_stops_owned_descendants_and_keeps_receipts(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            heartbeat = root / "heartbeat"
            child = root / "child.py"
            child.write_text("import pathlib, signal, sys, time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
                "while True:\n pathlib.Path(sys.argv[1]).write_text(str(time.monotonic()))\n time.sleep(.02)\n")
            command = [sys.executable, "-c",
                "import subprocess,sys,time; subprocess.Popen(sys.argv[1:]); time.sleep(60)",
                sys.executable, str(child), str(heartbeat)]
            receipt = root / "timing.jsonl"
            started = time.monotonic()
            self.assertEqual(runner.run_scene("crew", command, 1, receipt), 124)
            self.assertLess(time.monotonic() - started, 8)
            self.assertTrue(heartbeat.exists(), "The owned child must have actually started")
            time.sleep(.1)
            last = heartbeat.read_text()
            time.sleep(.15)
            self.assertEqual(heartbeat.read_text(), last)
            records = [json.loads(line) for line in receipt.read_text().splitlines()]
            self.assertEqual([r["state"] for r in records], ["started", "timed-out"])


if __name__ == "__main__":
    unittest.main()
