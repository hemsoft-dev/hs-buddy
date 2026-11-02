"""Synthetic failure controls for diagnostic gaps; no Mac input or real PTY."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import struct
import tempfile
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

settings = load("settings-desktop")
transport = load("terminal-transport")

class BoundaryEvidence(unittest.TestCase):
    def test_mac_snapshot_records_state_without_claiming_callback_delivery(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            config = root / "config.json"
            config.write_text(json.dumps(dict(notifications=dict(playSoundOnReviewComplete=False))))
            driver = object.__new__(settings.Driver)
            driver.config = config
            events = []
            window = Mock()
            window.screenshot.side_effect = lambda path: (events.append(path.name), path.write_bytes(b"owned synthetic frame"))
            with patch.dict(os.environ, GITHUB_ACTIONS="true", BUDDY_SETTINGS_FIXTURE="1"):
                driver.capture_click_boundary(window, root, "rightfield", "Organization", [980, 387])
                config.write_text(json.dumps(dict(notifications=dict(playSoundOnReviewComplete=True))))
                driver.capture_click_boundary(window, root, "content", "Review completion sound: off", [704, 290])
            rows = [json.loads(line) for line in (root / "click-boundaries.jsonl").read_text().splitlines()]
            self.assertEqual(events, ["after-click-000.png", "after-click-001.png"])
            self.assertEqual([row["soundEnabled"] for row in rows], [False, True])
            self.assertTrue(all(row["callbackDelivered"] is None and row["beforeTyping"] for row in rows))
            window.key.assert_not_called()

    def test_mac_capture_has_no_effect_outside_the_fixture(self):
        driver = object.__new__(settings.Driver)
        window = Mock()
        with patch.dict(os.environ, GITHUB_ACTIONS="false", BUDDY_SETTINGS_FIXTURE="1"):
            driver.capture_click_boundary(window, Path("unowned"), "input", "Organization", [1, 2])
        window.screenshot.assert_not_called()

    def test_actual_mac_interact_captures_before_replacement_without_retry(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            driver = object.__new__(settings.Driver)
            driver.session = "macos"
            driver.config = root / "config.json"
            driver.config.write_text("{}")
            events = []
            window, helper = Mock(), Mock()
            window.screenshot.side_effect = lambda path: (events.append("capture"), path.write_bytes(b"owned frame"))
            helper.click.side_effect = lambda *_: events.append("click")
            def replace(_text):
                events.append("type")
                raise RuntimeError("controlled typing failure")
            helper.replace_text.side_effect = replace
            driver.input_receipt = Mock(side_effect=lambda *_: events.append("receipt"))
            def locate(_window, image, *_args, **_kwargs):
                image.write_bytes(b"0" * 16 + struct.pack(">II", 1200, 820))
                image.with_suffix(".window.json").write_text(json.dumps(dict(
                    kCGWindowBounds=dict(X=80, Y=80, Width=1200, Height=820), capturePixelsPerWindowPoint=1)))
                return dict(left=300, top=270, width=60, height=12)
            driver.locate = Mock(side_effect=locate)
            with patch.dict(os.environ, GITHUB_ACTIONS="true", BUDDY_SETTINGS_FIXTURE="1"), \
                    patch.object(settings, "OwnedMacInput", return_value=helper), \
                    patch.object(settings, "run"), patch.object(settings.time, "sleep"):
                with self.assertRaisesRegex(RuntimeError, "controlled typing failure"):
                    driver.interact(window, root, "rightfield", ["Organization|GitHub username", "synthetic-team"])
            self.assertEqual(events, ["click", "receipt", "capture", "type"])
            helper.click.assert_called_once()
            self.assertTrue((root / "after-click-000.png").is_file())

    def test_actual_terminal_timeout_retains_bytes_without_a_pass_receipt(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            for name in ("terminal-smoke", "terminal-host"):
                binary = root / "target/debug/examples" / (name + (".exe" if os.name == "nt" else ""))
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_bytes(b"synthetic binary")
            timeout = subprocess.TimeoutExpired("owned driver", 180, output=b"ack\n", stderr=b"BUDDY_WAITING\n^C\n")
            with patch.dict(os.environ, GITHUB_ACTIONS="true", RUNNER_TEMP=str(root), CARGO_TARGET_DIR=str(root / "target")), \
                    patch.object(sys, "argv", ["transport", "--expected-source", "head"]), \
                    patch.object(transport.subprocess, "check_output", side_effect=["head", ""]), \
                    patch.object(transport.subprocess, "run", side_effect=timeout):
                with self.assertRaises(subprocess.TimeoutExpired) as failure:
                    transport.main()
            self.assertIs(failure.exception, timeout)
            evidence = root / "native-evidence/terminal-transport/core-example"
            self.assertEqual((evidence / "stdout.log").read_text(), "ack\n")
            self.assertEqual((evidence / "stderr.log").read_text(), "BUDDY_WAITING\n^C\n")
            self.assertFalse((evidence / "qualification.json").exists())

if __name__ == "__main__": unittest.main()
