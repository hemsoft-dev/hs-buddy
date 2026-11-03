"""Real click/post/receipt consumers, fake CoreGraphics only; not OS proof."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

inputs, settings = load("macos-input"), load("settings-desktop")

class ConstructedMouse(unittest.TestCase):
    def helper(self):
        value = object.__new__(inputs.OwnedMacInput)
        value.process, value.graphics, value.core = Mock(), Mock(), Mock()
        value.process.poll.return_value = None
        value.graphics.CGPreflightPostEventAccess.return_value = True
        value.release_modifiers = Mock()
        value.graphics.CGEventCreateMouseEvent.side_effect = lambda _s, kind, _p, _b: kind
        value.graphics.CGEventGetType.side_effect = lambda event: event
        value.graphics.CGEventGetLocation.side_effect = lambda event: inputs.Point(704.569, 290.5)
        value.graphics.CGEventGetFlags.return_value = 0
        value.graphics.CGEventGetIntegerValueField.side_effect = lambda event, field: 1 if field == 1 else 0
        return value

    def click(self, value):
        with patch.object(inputs.time, "sleep"):
            value.click(704.569, 290.5)
        self.assertEqual([call.args for call in value.graphics.CGEventPost.call_args_list], [(0, 5), (0, 1), (0, 2)])
        self.assertEqual([call.args for call in value.core.CFRelease.call_args_list], [(5,), (1,), (2,)])
        self.assertEqual(value.graphics.CGEventCreateMouseEvent.call_count, 3)

    def test_actual_created_event_readback_reaches_actual_receipt(self):
        value = self.helper()
        self.click(value)
        readings = [row["constructedEvent"] for row in value.last_click_trace]
        self.assertEqual([row["actualType"] for row in readings], ["move", "down", "up"])
        for i, row in enumerate(readings):
            self.assertEqual(set(row), {"status", "actualType", "typeMatches", "locationMatches", "leftButtonMatches", "clickStateMatches", "flagsClear"})
            self.assertEqual(row["status"], "observed")
            self.assertTrue(all(row[key] is True for key in ("typeMatches", "locationMatches", "leftButtonMatches", "flagsClear")))
            self.assertIs(row["clickStateMatches"], None if i == 0 else True)
        value.focus_state = Mock(return_value={})
        with tempfile.TemporaryDirectory() as raw:
            driver = object.__new__(settings.Driver)
            driver.input_receipt(value, Path(raw), "after-click", "content", "synthetic", [704.569, 290.5])
            rows = json.loads((Path(raw) / "input-state.jsonl").read_text())["clickTrace"]
        self.assertEqual([row["constructedEvent"] for row in rows], readings)

    def test_mismatched_properties_are_reported_without_changing_input(self):
        for getter, replacement, key in [
            ("CGEventGetType", lambda event: 777, "typeMatches"),
            ("CGEventGetLocation", lambda event: inputs.Point(1, 2), "locationMatches"),
            ("CGEventGetFlags", lambda event: 1, "flagsClear"),
            ("CGEventGetIntegerValueField", lambda event, field: 777, "leftButtonMatches"),
        ]:
            with self.subTest(getter=getter):
                value = self.helper()
                getattr(value.graphics, getter).side_effect = replacement
                self.click(value)
                for row in value.last_click_trace:
                    self.assertIs(row["constructedEvent"][key], False)
                if getter == "CGEventGetType":
                    self.assertEqual({row["constructedEvent"]["actualType"] for row in value.last_click_trace}, {"other"})
        value = self.helper()
        value.graphics.CGEventGetIntegerValueField.side_effect = lambda event, field: 0
        self.click(value)
        self.assertEqual([row["constructedEvent"]["clickStateMatches"] for row in value.last_click_trace], [None, False, False])

    def test_every_readback_failure_preserves_required_up_and_release(self):
        for getter in ("CGEventGetType", "CGEventGetLocation", "CGEventGetFlags", "CGEventGetIntegerValueField"):
            with self.subTest(getter=getter):
                value = self.helper()
                getattr(value.graphics, getter).side_effect = RuntimeError("sensitive error must never be retained")
                self.click(value)
                self.assertEqual([row["constructedEvent"] for row in value.last_click_trace], [{"status": "unavailable"}] * 3)

    def test_post_failure_keeps_original_exception_and_releases_event(self):
        value = self.helper()
        failure = RuntimeError("original post failure")
        value.graphics.CGEventGetType.side_effect = ValueError("ancillary failure")
        value.graphics.CGEventPost.side_effect = failure
        with self.assertRaises(RuntimeError) as caught:
            value.post(1, mouse=(1, 704.569, 290.5))
        self.assertIs(caught.exception, failure)
        value.core.CFRelease.assert_called_once_with(1)

    def test_non_mouse_post_does_not_read_mouse_properties(self):
        value = self.helper()
        value.post(42)
        for getter in ("CGEventGetType", "CGEventGetLocation", "CGEventGetFlags", "CGEventGetIntegerValueField"):
            getattr(value.graphics, getter).assert_not_called()

    def test_readback_uses_live_object_after_flags_and_before_post_release(self):
        value = self.helper()
        order = []
        value.graphics.CGEventSetFlags.side_effect = lambda event, flags: order.append(("flags", event, flags))
        def get_type(event):
            order.append(("read", event))
            return event
        value.graphics.CGEventGetType.side_effect = get_type
        value.graphics.CGEventPost.side_effect = lambda tap, event: order.append(("post", event))
        value.core.CFRelease.side_effect = lambda event: order.append(("release", event))
        self.click(value)
        self.assertEqual(order, [row for event in (5, 1, 2) for row in
            (("flags", event, 0), ("read", event), ("post", event), ("release", event))])

    def test_real_constructor_binds_pointer_and_result_widths_with_fake_libraries(self):
        process = Mock(pid=123)
        process.poll.return_value = None
        graphics, core = Mock(), Mock()
        with patch.object(inputs.c, "CDLL", side_effect=[graphics, core]), patch.dict(
                "os.environ", GITHUB_ACTIONS="true", BUDDY_SETTINGS_FIXTURE="1"):
            inputs.OwnedMacInput(process)
        for name, result in (("CGEventGetType", inputs.c.c_uint32),
                ("CGEventGetFlags", inputs.c.c_uint64), ("CGEventGetLocation", inputs.Point)):
            self.assertEqual(getattr(graphics, name).argtypes, [inputs.c.c_void_p])
            self.assertIs(getattr(graphics, name).restype, result)
        self.assertEqual(graphics.CGEventGetIntegerValueField.argtypes, [inputs.c.c_void_p, inputs.c.c_uint32])
        self.assertIs(graphics.CGEventGetIntegerValueField.restype, inputs.c.c_int64)
