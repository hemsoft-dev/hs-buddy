"""Actual posting/receipt/camera consumers with fake OS; no UI or input."""
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import Mock, patch
import desktop

def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

inputs, settings = load("macos-input"), load("settings-desktop")

class MacReceiptTiming(unittest.TestCase):
    def helper(self):
        helper = object.__new__(inputs.OwnedMacInput)
        helper.process = Mock(pid=123)
        helper.process.poll.return_value = None
        helper.graphics, helper.core = Mock(), Mock()
        helper.graphics.CGPreflightPostEventAccess.return_value = True
        helper.graphics.CGEventSourceButtonState.return_value = False
        helper.graphics.CGEventCreateMouseEvent.side_effect = lambda _source, kind, _point, _button: kind
        helper.release_modifiers = Mock()
        return helper

    def assert_span(self, span):
        self.assertEqual(set(span), {"start", "end"})
        for endpoint in span.values():
            self.assertEqual(set(endpoint), {"wallMillis", "monotonicNanos"})
            self.assertTrue(all(type(value) is int for value in endpoint.values()))
        self.assertLessEqual(span["start"]["monotonicNanos"], span["end"]["monotonicNanos"])

    def test_actual_single_click_and_receipt_retain_post_and_observer_spans(self):
        helper = self.helper()
        with patch.object(inputs.time, "sleep"):
            helper.click(704, 290)
        self.assertEqual([row["eventType"] for row in helper.last_click_trace], [5, 1, 2])
        self.assertEqual(helper.graphics.CGEventPost.call_count, 3)
        for row in helper.last_click_trace:
            self.assert_span(row["postCall"])
        helper.focus_state = Mock(return_value={"owned": True})
        with tempfile.TemporaryDirectory() as raw:
            driver = object.__new__(settings.Driver)
            driver.input_receipt(helper, Path(raw), "after-click", "content", "synthetic", [704, 290])
            receipt = json.loads((Path(raw) / "input-state.jsonl").read_text())
        self.assert_span(receipt["focusObservation"])
        self.assertEqual(receipt["clickTrace"], helper.last_click_trace)
        self.assertNotIn("delivered", receipt)

    def test_post_exception_retains_failed_call_span_and_original_exception(self):
        helper = self.helper()
        failure = RuntimeError("controlled boundary failure")
        helper.graphics.CGEventPost.side_effect = failure
        with self.assertRaises(RuntimeError) as raised:
            helper.post(5)
        self.assertIs(raised.exception, failure)
        self.assert_span(helper.last_post_timing)
        helper.core.CFRelease.assert_called_once_with(5)

    def test_actual_camera_metadata_retains_capture_interval_without_extra_commands(self):
        with tempfile.TemporaryDirectory() as raw:
            image = Path(raw) / "frame.png"
            window = desktop.UnixWindow(Mock(pid=123), "macos")
            def run(*args, **kwargs):
                if args[0] == "screencapture":
                    image.write_bytes(b"0" * 16 + struct.pack(">II", 1200, 820))
                else:
                    return json.dumps(dict(kCGWindowNumber=9, kCGWindowBounds=dict(Width=1200, Height=820)))
            with patch.object(desktop, "run", side_effect=run) as command, patch.dict("os.environ", RUNNER_TEMP=raw):
                window.screenshot(image)
            self.assertEqual(command.call_count, 2)
            value = json.loads(image.with_suffix(".window.json").read_text())
            self.assert_span(value["captureTiming"])
            self.assertEqual(value["capturePixelSize"], [1200, 820])

    def test_wall_clock_adjustment_is_retained_not_normalized_into_order(self):
        helper = self.helper()
        start = dict(wallMillis=200, monotonicNanos=10)
        end = dict(wallMillis=100, monotonicNanos=20)
        with patch.object(inputs, "stamp", side_effect=[start, end]):
            helper.post(5)
        self.assertEqual(helper.last_post_timing, dict(start=start, end=end))
        self.assertLess(helper.last_post_timing["end"]["wallMillis"], helper.last_post_timing["start"]["wallMillis"])
