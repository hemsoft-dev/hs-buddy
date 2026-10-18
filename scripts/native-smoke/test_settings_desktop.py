"""Choose supported fixtures before requesting sizes; never accept clamping."""
import importlib.util
import base64
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import unittest
from unittest.mock import MagicMock, Mock, patch

spec = importlib.util.spec_from_file_location("settings_desktop", Path(__file__).with_name("settings-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class CompanionHandshake(unittest.TestCase):
    def test_preparation_retires_old_signals_and_stop_echoes_only_current_readiness(self):
        with tempfile.TemporaryDirectory(prefix="buddy-companion-protocol-") as root:
            profile = Path(root)
            (profile / ".buddy-settings-fixture").write_text("buddy settings fixture v1")
            for name in ("ready", "stop", "complete", "failed"):
                (profile / f"companion-{name}.json").write_text('{"runId":"stale"}')
            request = desktop.prepare_companion(profile)
            self.assertFalse((profile / "companion-stop.json").exists())
            self.assertFalse((profile / "companion-complete.json").exists())
            (profile / "companion-ready.json").write_text(json.dumps(request))
            desktop.stop_companion(profile)
            self.assertEqual(json.loads((profile / "companion-stop.json").read_text()), request)

    def test_another_runs_readiness_cannot_receive_a_stop_signal(self):
        with tempfile.TemporaryDirectory(prefix="buddy-companion-protocol-") as root:
            profile = Path(root)
            (profile / "companion-request.json").write_text('{"runId":"current"}')
            (profile / "companion-ready.json").write_text('{"runId":"other"}')
            with self.assertRaisesRegex(RuntimeError, "another run"):
                desktop.stop_companion(profile)
            self.assertFalse((profile / "companion-stop.json").exists())

    def test_receipt_requires_current_run_and_observed_writes_on_both_sides(self):
        ready = dict(runId="current", startedAt=100)
        receipt = dict(runId="current", completedAt=30_000, observedNativeSaveAt=25_000,
            writesBeforeNativeSave=5, writesAfterNativeSave=2)
        desktop.verify_companion_overlap(ready, receipt, 26_000)
        for bad in (dict(runId="other"), dict(observedNativeSaveAt=None),
                dict(observedNativeSaveAt=99), dict(writesBeforeNativeSave=0),
                dict(writesAfterNativeSave=0), dict(completedAt=24_000)):
            with self.subTest(bad=bad), self.assertRaisesRegex(RuntimeError, "not observed"):
                desktop.verify_companion_overlap(ready, {**receipt, **bad}, 26_000)
        with self.assertRaisesRegex(RuntimeError, "not observed"):
            desktop.verify_companion_overlap(ready, receipt, 30_001)

    def test_preparation_requires_the_owned_fixture_marker(self):
        with tempfile.TemporaryDirectory(prefix="buddy-companion-protocol-") as root:
            profile = Path(root)
            (profile / ".buddy-settings-fixture").write_text("not owned")
            with self.assertRaisesRegex(RuntimeError, "Owned settings fixture"):
                desktop.prepare_companion(profile)


class DesktopLimits(unittest.TestCase):
    def test_compact_mac_reveal_posts_two_small_line_events_before_the_next_ocr_attempt(self):
        driver = object.__new__(desktop.Driver)
        driver.session = 'macos'
        owned = object.__new__(desktop.OwnedMacInput)
        owned.require_focus = Mock()
        owned.graphics = Mock()
        owned.post = Mock()
        metadata = json.dumps(dict(kCGWindowBounds=dict(X=80, Y=80, Width=900, Height=650)))
        with patch.dict(os.environ, {'RUNNER_TEMP': 'owned-fixture'}), \
                patch.object(desktop, 'run', return_value=metadata), \
                patch.object(desktop, 'OwnedMacInput', return_value=owned), \
                patch.object(desktop.time, 'sleep'):
            driver.reveal_scroll(Mock(), (90, 357))
        # The actual Intel capture moved by just one line before slow OCR
        # exhausted the ordinary wait. Keep each event small, but send both
        # before taking the next frame and requiring the exact visible status.
        self.assertEqual([call.args for call in owned.graphics.CGEventCreateScrollWheelEvent.call_args_list],
            [(None, 1, 1, -1), (None, 1, 1, -1)])
        self.assertEqual(owned.post.call_count, 2)
        self.assertEqual(owned.require_focus.call_count, 2)
        self.assertEqual([(call.args[1].x, call.args[1].y) for call in owned.graphics.CGEventSetLocation.call_args_list],
            [(170, 437), (170, 437)])
        driver.session = 'x11'
        with patch.object(desktop, 'run') as run, patch.object(desktop.time, 'sleep'):
            driver.reveal_scroll(Mock(window='owned-window'), (90, 357))
        self.assertEqual([call.args for call in run.call_args_list], [
            ('xdotool', 'mousemove', '--window', 'owned-window', 90, 357),
            ('xdotool', 'click', '--repeat', 1, '--delay', 50, 5),
        ])

    def test_anchored_mac_scroll_uses_one_line_while_legacy_scroll_keeps_its_step(self):
        driver = object.__new__(desktop.Driver)
        driver.session = "macos"
        window = Mock()
        owned = object.__new__(desktop.OwnedMacInput)
        owned.require_focus = Mock()
        owned.graphics = Mock()
        owned.post = Mock()
        metadata = json.dumps(dict(kCGWindowBounds=dict(X=10, Y=20, Width=900, Height=650)))
        with patch.dict(os.environ, {"RUNNER_TEMP": "owned-fixture"}), \
                patch.object(desktop, "run", return_value=metadata), \
                patch.object(desktop, "OwnedMacInput", return_value=owned), \
                patch.object(desktop.time, "sleep"):
            for point, precise, expected, position in [
                    ((95, 360), None, -1, (105, 380)),
                    (None, None, -10, (460, 345)),
                    ((95, 360), False, -10, (105, 380))]:
                owned.graphics.reset_mock()
                driver.scroll(window, -120, point, precise=precise)
                self.assertEqual(owned.graphics.CGEventCreateScrollWheelEvent.call_args.args, (None, 1, 1, expected))
                observed = owned.graphics.CGEventSetLocation.call_args.args[1]
                self.assertEqual((observed.x, observed.y), position)

    def test_long_session_scroll_requires_the_owned_pane_and_retains_coarse_steps(self):
        driver = object.__new__(desktop.Driver)
        driver.session = 'macos'
        # Actual Intel 900x650 capture: default centre (450,325) is above
        # the detail viewport at y327. Its visible metrics are inside it.
        driver.scroll_anchor = Mock(return_value=(570, 444))
        state = {'progress': 0}

        def scroll(_window, delta, point=None, *, precise=None):
            x, y = point if point is not None else (450, 325)
            if 340 <= x < 884 and 327 <= y < 535 and delta < 0 and precise is False:
                state['progress'] += 1

        driver.scroll = Mock(side_effect=scroll)

        def observe(_window, _directory, text, *, scroll):
            self.assertEqual(text, 'Inspect synthetic request 20')
            for _ in range(2):
                scroll()
            if state['progress'] != 2:
                raise desktop.MissingVisibleText('The request did not enter the owned viewport')

        window, directory = Mock(), Path('owned-capture')
        with patch.object(desktop, 'wait_visible', side_effect=observe):
            driver.wait_scrolled(window, directory, 'Inspect synthetic request 20', 'Output / prompt tokens')
        driver.scroll_anchor.assert_called_once_with(window, directory, 'Output / prompt tokens')
        self.assertEqual(state['progress'], 2)

    def test_slow_ocr_does_not_starve_scroll_to_the_last_request(self):
        for never_visible in (False, True):
            with self.subTest(never_visible=never_visible):
                driver = object.__new__(desktop.Driver)
                driver.session = 'macos'
                clock = {'now': 0.}
                moves = []

                def anchor(*_args, **_kwargs):
                    clock['now'] += 2
                    return 537.5, 530

                def scroll(_window, delta, point=None, *, precise=None):
                    self.assertEqual((delta, point, precise), (-1200, (537.5, 530), False))
                    moves.append(delta)
                    clock['now'] += .7

                def recognize(_image, text):
                    self.assertEqual(text, 'Inspect synthetic request 20')
                    if never_visible or len(moves) < 2:
                        # Retained Intel OCR outputs span about eight seconds.
                        clock['now'] += 8
                        raise RuntimeError('The last request is still offscreen')
                    clock['now'] += .4
                    return dict(left=341, top=450, width=161, height=13)

                driver.scroll_anchor = anchor
                driver.scroll = scroll
                with patch.object(desktop.ocr, 'locate_image', side_effect=recognize), \
                        patch.object(desktop.time, 'monotonic', side_effect=lambda: clock['now']), \
                        patch.object(desktop.time, 'sleep', side_effect=lambda seconds: clock.__setitem__('now', clock['now'] + seconds)):
                    if never_visible:
                        with self.assertRaisesRegex(RuntimeError, 'did not show the expected status'):
                            driver.wait_scrolled(Mock(), Path('owned-capture'), 'Inspect synthetic request 20', 'Next requests', above_anchor=True)
                        self.assertGreaterEqual(clock['now'], 15)
                    else:
                        driver.wait_scrolled(Mock(), Path('owned-capture'), 'Inspect synthetic request 20', 'Next requests', above_anchor=True)
                        self.assertEqual(len(moves), 2)
                        self.assertLess(clock['now'], 15)

    def test_an_already_visible_request_does_not_measure_an_offscreen_anchor_or_scroll(self):
        driver = object.__new__(desktop.Driver)
        driver.session = 'macos'
        driver.scroll_anchor = Mock(side_effect=AssertionError('No scroll needed'))
        driver.scroll = Mock()
        with patch.object(desktop.ocr, 'locate_image', return_value=dict(left=340, top=450, width=180, height=13)):
            driver.wait_scrolled(Mock(), Path('owned-capture'), 'Final synthetic request twenty one', 'Request 21')
        driver.scroll_anchor.assert_not_called()
        driver.scroll.assert_not_called()

    def test_unanchored_scroll_waits_keep_the_existing_default_target(self):
        driver = object.__new__(desktop.Driver)
        driver.scroll_anchor = Mock()
        driver.scroll = Mock()
        window = Mock()
        with patch.object(desktop, 'wait_visible', side_effect=lambda _w, _d, _t, *, scroll: scroll()):
            driver.wait_scrolled(window, Path('owned-capture'), 'fixture-member')
        driver.scroll.assert_called_once_with(window, -1200)
        driver.scroll_anchor.assert_not_called()

    def test_scroll_anchor_uses_the_measured_label_instead_of_the_old_wheel_position(self):
        driver = object.__new__(desktop.Driver)
        driver.session = "x11"
        driver.locate = Mock(return_value=dict(left=60, top=350, width=70, height=20))
        point = driver.scroll_anchor(Mock(), Path("owned-capture"), "Assistant")
        self.assertEqual(point, (95, 360))
        window = Mock(window="owned-window")
        with patch.object(desktop, "run") as run, patch.object(desktop.time, "sleep"):
            driver.scroll(window, -120, point)
        self.assertEqual(run.call_args_list[0].args, ("xdotool", "mousemove", "--window", "owned-window", 95, 360))

    def test_mac_scroll_anchor_converts_capture_pixels_with_the_observed_scale(self):
        driver = object.__new__(desktop.Driver)
        driver.session = "macos"
        driver.locate = Mock(return_value=dict(left=120, top=700, width=140, height=40))
        with tempfile.TemporaryDirectory(prefix="buddy-scroll-anchor-") as root:
            directory = Path(root)
            (directory / "settings-scroll-anchor.window.json").write_text(json.dumps(dict(capturePixelsPerWindowPoint=2)))
            self.assertEqual(driver.scroll_anchor(Mock(), directory, "Assistant"), (95, 360))

    def test_mac_scroll_anchor_requires_its_capture_calibration(self):
        driver = object.__new__(desktop.Driver)
        driver.session = "macos"
        driver.locate = Mock(return_value=dict(left=120, top=700, width=140, height=40))
        with tempfile.TemporaryDirectory(prefix="buddy-scroll-anchor-") as root:
            with self.assertRaises(FileNotFoundError):
                driver.scroll_anchor(Mock(), Path(root), "Assistant")

    def test_fixed_pager_measures_inside_the_pane_at_both_capture_scales(self):
        driver = object.__new__(desktop.Driver)
        driver.session = "macos"
        for scale in (1, 2):
            with self.subTest(scale=scale), tempfile.TemporaryDirectory() as root:
                directory = Path(root)
                (directory / "settings-scroll-anchor.window.json").write_text(json.dumps(dict(capturePixelsPerWindowPoint=scale)))
                # Native Vision output for the retained 900x650 Intel capture.
                driver.locate = Mock(return_value=dict(left=490*scale, top=556*scale, width=92*scale, height=16*scale))
                point = driver.scroll_anchor(Mock(), directory, "Next requests", above=True)
                self.assertEqual(point, (536, 524))
                self.assertTrue(340 <= point[0] < 884 and 327 <= point[1] < 535)
                self.assertLess(point[1], 547)  # The button's outer top.

    def test_pager_scrolling_is_lazy_coarse_and_requires_a_measured_control(self):
        driver = object.__new__(desktop.Driver)
        driver.session = 'x11'
        driver.scroll_anchor = Mock(return_value=(536, 524))
        driver.scroll = Mock()
        window, directory = Mock(), Path("owned-capture")
        with patch.object(desktop, "wait_visible", side_effect=lambda _w, _d, _t, *, scroll: (scroll(), scroll())):
            driver.wait_scrolled(window, directory, "Inspect synthetic request 20", "Next requests", above_anchor=True)
        driver.scroll_anchor.assert_called_once_with(window, directory, "Next requests", above=True)
        self.assertEqual(driver.scroll.call_count, 2)
        driver.scroll.assert_called_with(window, -1200, (536, 524), precise=False)
        with self.assertRaises(RuntimeError):
            driver.wait_scrolled(window, directory, "Missing", above_anchor=True)

    def test_invalid_pager_geometry_cannot_produce_an_input_point(self):
        driver = object.__new__(desktop.Driver)
        driver.session = "x11"
        for top, height in ((10, 16), (556, 0), (float("nan"), 16)):
            with self.subTest(top=top, height=height), self.assertRaises(RuntimeError):
                driver.locate = Mock(return_value=dict(left=490, top=top, width=92, height=height))
                driver.scroll_anchor(Mock(), Path("owned-capture"), "Next requests", above=True)

    def test_full_and_small_host_limits_are_used_before_resize(self):
        self.assertEqual(desktop.planned_large_size(1920, 1080), (1200, 820))
        self.assertEqual(desktop.planned_large_size(1044, 788), (1044, 788))
        self.assertEqual(desktop.planned_large_size(1024, 683), (1024, 683))

    def test_a_host_below_the_supported_minimum_fails(self):
        with self.assertRaises(RuntimeError):
            desktop.planned_large_size(899, 820)
        with self.assertRaises(RuntimeError):
            desktop.planned_large_size(1200, 649)


class ColorReadVerification(unittest.TestCase):
    def capture(self):
        path = Mock()
        path.read_bytes.return_value = bytes(16) + struct.pack(">II", 1200, 820)
        path.with_suffix.return_value.read_text.return_value = json.dumps(dict(capturePixelsPerWindowPoint=1))
        return path

    def test_wrong_hex_and_misread_glyphs_are_never_accepted_as_the_expected_color(self):
        label = dict(left=72, save=dict(left=1134, top=367, height=10))
        for actual in ("#8246a1", "#8246al"):
            with self.subTest(actual=actual), patch.object(desktop.ocr, "locate_image", return_value=label), \
                    patch.object(desktop, "run", side_effect=["", actual, actual]):
                with self.assertRaisesRegex(RuntimeError, "exact saved hex"):
                    desktop.verify_color(self.capture(), "#8246af", "macos")

    def test_a_second_line_mode_must_read_the_exact_value(self):
        label = dict(left=72, save=dict(left=1134, top=367, height=10))
        with patch.object(desktop.ocr, "locate_image", return_value=label), \
                patch.object(desktop, "run", side_effect=["", "#8246al", "#8246af"]):
            desktop.verify_color(self.capture(), "#8246af", "macos")


class DraftReadVerification(unittest.TestCase):
    capture = ColorReadVerification.capture

    def test_wrong_glyphs_and_additional_text_are_rejected(self):
        label = dict(left=73, save=dict(left=1133, top=281, height=11))
        for actual in ("draft audlo", "draft audio extra", "; draft audio ;"):
            with self.subTest(actual=actual), patch.object(desktop.ocr, "locate_image", return_value=label), \
                    patch.object(desktop, "run", side_effect=["", actual, actual]):
                with self.assertRaisesRegex(RuntimeError, "exact retained draft"):
                    desktop.verify_draft(self.capture(), "Notification audio file", "draft audio")

    def test_second_mode_requires_the_exact_unaliased_value(self):
        label = dict(left=73, save=dict(left=1133, top=281, height=11))
        with patch.object(desktop.ocr, "locate_image", return_value=label), \
                patch.object(desktop, "run", side_effect=["", "draft audlo", "draft audio"]):
            desktop.verify_draft(self.capture(), "Notification audio file", "draft audio")


class LabeledInputs(unittest.TestCase):
    def test_cold_model_wait_requires_the_catalog_and_still_expires(self):
        window, directory = Mock(), MagicMock()
        with patch.object(desktop.ocr, "locate_image", side_effect=[RuntimeError("loading"), dict(left=10)]), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 16]), patch.object(desktop.time, "sleep"):
            desktop.wait_visible(window, directory, "Synthetic offline model", timeout=desktop.MODEL_CATALOG_TIMEOUT)
        self.assertEqual(window.screenshot.call_count, 2)
        window.key.assert_not_called()
        with patch.object(desktop.ocr, "locate_image", side_effect=RuntimeError("still loading")), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 199, 201]), patch.object(desktop.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "did not show the expected status"):
                desktop.wait_visible(window, directory, "Synthetic offline model", timeout=desktop.MODEL_CATALOG_TIMEOUT)
        window.key.assert_not_called()

    def test_windows_cold_model_budget_does_not_change_ordinary_status_commands(self):
        with tempfile.TemporaryDirectory(prefix="buddy-model-protocol-") as root:
            driver = desktop.Driver.__new__(desktop.Driver)
            driver.profile = Path(root)
            driver.config = Path(root) / "config.json"
            driver.executable = Path(root) / "owned-app.exe"
            driver.checkout = Path(root)
            driver.session = "windows"
            driver.large_size = (1044, 788)
            driver.backend = Mock(url="http://127.0.0.1:12345")
            with patch.object(desktop, "run") as execute:
                driver.drive("catalog", [("wait_models", "Synthetic offline model"),
                    ("wait_text", "Saved to results"), ("key", "Return"),
                    ("wait_scroll_above", "Inspect synthetic request 20", "Next requests")])
            args = execute.call_args.args
            commands = args[args.index("-Actions") + 1].split(",")
            self.assertEqual(commands[1], "waitmodeltext64:" + desktop.b64("Synthetic offline model"))
            self.assertEqual(commands[2], "waittext64:" + desktop.b64("Saved to results"))
            self.assertEqual(commands[3], "key:13")
            self.assertEqual(commands[4], "waitscrolltext64:" + desktop.b64("Inspect synthetic request 20"))
            self.assertEqual(execute.call_args.kwargs["timeout"], 180 + desktop.MODEL_CATALOG_TIMEOUT - 15)

    def test_scrolling_reveals_feedback_but_requires_a_new_observation(self):
        window, directory, scroll = Mock(), MagicMock(), Mock()
        with patch.object(desktop.ocr, "locate_image", side_effect=[RuntimeError("offscreen"), dict(left=10)]), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 1]), patch.object(desktop.time, "sleep"):
            desktop.wait_visible(window, directory, "Saved to results", scroll=scroll)
        self.assertEqual(window.screenshot.call_count, 2)
        scroll.assert_called_once_with()
        window.key.assert_not_called()

    def test_scroll_success_never_substitutes_for_missing_feedback(self):
        window, directory, scroll = Mock(), MagicMock(), Mock()
        with patch.object(desktop.ocr, "locate_image", side_effect=RuntimeError("absent")), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 1, 16]), patch.object(desktop.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "did not show the expected status"):
                desktop.wait_visible(window, directory, "Saved to results", scroll=scroll)
        self.assertEqual(window.screenshot.call_count, 2)
        scroll.assert_called_once_with()

    def test_visible_feedback_does_not_move_the_conversation(self):
        scroll = Mock()
        with patch.object(desktop.ocr, "locate_image", return_value=dict(left=10)):
            desktop.wait_visible(Mock(), MagicMock(), "Saved to results", scroll=scroll)
        scroll.assert_not_called()

    def test_status_barrier_waits_for_an_observed_acknowledgment_without_posting_input(self):
        window, directory = Mock(), MagicMock()
        with patch.object(desktop.ocr, "locate_image", side_effect=[RuntimeError("not ready"), dict(left=10)]) as locate, \
                patch.object(desktop.time, "monotonic", side_effect=[0, 1]), patch.object(desktop.time, "sleep"):
            desktop.wait_visible(window, directory, "Paste written. No Enter key was added.")
        self.assertEqual(window.screenshot.call_count, 2)
        self.assertEqual(locate.call_count, 2)
        window.key.assert_not_called()

    def test_missing_acknowledgment_times_out_without_posting_return(self):
        window, directory = Mock(), MagicMock()
        with patch.object(desktop.ocr, "locate_image", side_effect=RuntimeError("not ready")), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 16]), patch.object(desktop.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "did not show the expected status"):
                desktop.wait_visible(window, directory, "Paste written. No Enter key was added.")
        window.key.assert_not_called()

    def test_status_capture_failure_is_not_retried_as_an_absent_label(self):
        window = Mock()
        window.screenshot.side_effect = OSError("capture failed")
        with patch.object(desktop.ocr, "locate_image") as locate:
            with self.assertRaisesRegex(OSError, "capture failed"):
                desktop.wait_visible(window, MagicMock(), "NATIVEREADY")
            locate.assert_not_called()
        window.key.assert_not_called()

    def test_windows_sequence_waits_for_paste_acknowledgment_before_return_and_output_capture(self):
        with tempfile.TemporaryDirectory() as root:
            driver = desktop.Driver.__new__(desktop.Driver)
            driver.profile = Path(root)
            driver.config = Path(root) / "config.json"
            driver.executable = Path(root) / "owned-app.exe"
            driver.checkout = Path(root)
            driver.session = "windows"
            driver.large_size = (1200, 820)
            driver.backend = Mock(url="http://127.0.0.1:12345")
            with patch.object(desktop, "run") as execute:
                driver.drive("acknowledged", [("click", "Insert + copy"),
                    ("wait_text", "Paste written. No Enter key was added."), ("key", "Return"),
                    ("wait_text", "NATIVEREADY"), ("capture", "result")])
            args = execute.call_args.args
            commands = args[args.index("-Actions") + 1].split(",")
            self.assertEqual([command.split(":")[0] for command in commands],
                ["resize", "click64", "waittext64", "key", "waittext64", "capture", "ctrlkey"])
            self.assertEqual(commands[2], "waittext64:" + desktop.b64("Paste written. No Enter key was added."))
            self.assertEqual(commands[3], "key:13")
            self.assertEqual(commands[4], "waittext64:" + desktop.b64("NATIVEREADY"))
            self.assertEqual(execute.call_args.kwargs["timeout"], 180)

    def test_tab_click_uses_the_toolbar_instead_of_matching_body_help_text(self):
        def word(text, left, top, line, width=40):
            return dict(text=text, left=left, top=top, line=line, width=width, height=12)
        words = [word("Filter", 200, 170, "1"), word("loaded", 245, 170, "1"),
            word("items", 290, 170, "1"), word("Reviews", 200, 245, "2", 60),
            word("Check", 200, 480, "3"), word("Reviews", 245, 480, "3", 60),
            word("and", 310, 480, "3"), word("Conversation", 355, 480, "3", 90)]
        driver = desktop.Driver.__new__(desktop.Driver)
        driver.session = "x11"
        directory = MagicMock()
        image = directory.__truediv__.return_value
        image.read_bytes.return_value = bytes(16) + struct.pack(">II", 1200, 820)
        window = Mock()
        with patch.object(desktop.ocr, "recognize", return_value=words), \
                patch.object(desktop, "run") as run, patch.object(desktop.time, "sleep"):
            driver.interact(window, directory, "tab", ["Reviews"])
        self.assertIn(unittest.mock.call("xdotool", "mousemove", "--window",
            window.window, 230, 251, "click", 1), run.call_args_list)

    def test_input_without_save_uses_its_label_and_replaces_text(self):
        for session, scale in (("macos", 2), ("x11", 1)):
            with self.subTest(session=session):
                driver = desktop.Driver.__new__(desktop.Driver)
                driver.session = session
                directory = MagicMock()
                image = directory.__truediv__.return_value
                image.read_bytes.return_value = bytes(16) + struct.pack(">II", 1200 * scale, 820 * scale)
                image.with_suffix.return_value.read_text.return_value = json.dumps(dict(
                    kCGWindowBounds=dict(X=100, Y=50), capturePixelsPerWindowPoint=scale))
                label = dict(left=800 * scale, top=100 * scale, width=120 * scale, height=14 * scale)
                window, helper = Mock(), Mock()
                helper.focus_state.return_value = dict(flags="owned fixture window")
                with patch.object(desktop.ocr, "locate_image", return_value=label) as locate, \
                        patch.object(desktop, "OwnedMacInput", return_value=helper), \
                        patch.object(desktop, "run") as run, patch.object(desktop.time, "sleep"):
                    driver.interact(window, directory, "input", ["Filter loaded items", "First private"])
                self.assertEqual(locate.call_count, 2 if session == "macos" else 1)
                locate.assert_called_with(image, "Filter loaded items", navigation=False,
                    field=True, save_row=False, tab=False)
                if session == "macos":
                    helper.click.assert_called_once_with(920, 184)
                    helper.replace_text.assert_called_once_with("First private")
                else:
                    self.assertIn(unittest.mock.call("xdotool", "mousemove", "--window",
                        window.window, 820, 134, "click", 1), run.call_args_list)
                    window.key.assert_called_once_with("a")
                    self.assertIn(unittest.mock.call("xdotool", "type", "--clearmodifiers",
                        "--delay", 10, "--", "First private"), run.call_args_list)


class SessionInputRows(unittest.TestCase):
    def test_spaced_form_hint_does_not_move_compact_workspace_inputs(self):
        for hint, expected in ((None, 326), ({"labelBottomOffset": 32}, 338)):
            with self.subTest(hint=hint):
                driver = desktop.Driver.__new__(desktop.Driver)
                driver.session = "x11"
                directory = MagicMock()
                directory.__truediv__.return_value.read_bytes.return_value = bytes(16) + struct.pack(">II",1200,820)
                window = Mock()
                values = ["Workspace name", "Scratch workspace"]
                if hint is not None:
                    values.append(hint)
                with patch.object(desktop.ocr,"locate_image",return_value=dict(left=64,top=295,width=110,height=11)) as locate, \
                        patch.object(desktop,"run") as execute, patch.object(desktop.time,"sleep"):
                    driver.interact(window,directory,"input",values)
                self.assertNotIn("row_control",locate.call_args.kwargs)
                clicks=[call.args for call in execute.call_args_list if call.args[:2]==("xdotool","mousemove")]
                self.assertEqual(clicks[0][4:6],(84,expected))

    def test_stacked_todoist_token_targets_the_input_interior_without_a_disabled_save_anchor(self):
        # Original installed X11 capture: label y295..306, input y324..356,
        # text interior y334..346. The disabled Save button below y401 has no
        # OCR glyphs and cannot locate this input's row.
        driver = desktop.Driver.__new__(desktop.Driver)
        driver.session = "x11"
        directory = MagicMock()
        image = directory.__truediv__.return_value
        image.read_bytes.return_value = bytes(16) + struct.pack(">II",1200,820)
        window = Mock()
        with patch.object(desktop.ocr,"locate_image",return_value=dict(left=64,top=295,width=110,height=11)) as locate, \
                patch.object(desktop,"run") as execute, patch.object(desktop.time,"sleep"):
            driver.interact(window,directory,"input",["Todoist API token","public synthetic token",{"labelBottomOffset":32}])
        self.assertNotIn("row_control",locate.call_args.kwargs)
        clicks=[call.args for call in execute.call_args_list if call.args[:2]==("xdotool","mousemove")]
        self.assertEqual(len(clicks),1)
        x,y=clicks[0][4:6]
        self.assertTrue(76<x<1172 and 334<=y<=346)

    def test_session_input_uses_the_measured_button_row_instead_of_border_padding(self):
        # Actual Intel failure: label bottom190 produced y210; the adjacent
        # Scan folder glyph spans215..225 and centers the editor at y220.
        for session, scale in (("macos", 1), ("macos", 2), ("x11", 1)):
            with self.subTest(session=session, scale=scale):
                driver = desktop.Driver.__new__(desktop.Driver)
                driver.session = session
                directory = MagicMock()
                image = directory.__truediv__.return_value
                image.read_bytes.return_value = bytes(16) + struct.pack(">II", 1200 * scale, 820 * scale)
                image.with_suffix.return_value.read_text.return_value = json.dumps(dict(
                    kCGWindowBounds=dict(X=100, Y=50), capturePixelsPerWindowPoint=scale))
                label = dict(left=65 * scale, top=177 * scale, width=135 * scale, height=13 * scale,
                    row_control=dict(left=1103 * scale, top=215 * scale, width=70 * scale, height=10 * scale))
                window, helper = Mock(), Mock()
                helper.focus_state.return_value = dict(flags="owned fixture window")
                with patch.object(desktop.ocr, "locate_image", return_value=label) as locate, \
                        patch.object(desktop, "OwnedMacInput", return_value=helper), \
                        patch.object(desktop, "run") as run, patch.object(desktop.time, "sleep"):
                    driver.interact(window, directory, "input", ["VS Code User directory", "synthetic path", "Scan folder"])
                locate.assert_called_with(image, "VS Code User directory", navigation=False,
                    field=True, save_row=False, tab=False, row_control="Scan folder")
                if session == "macos":
                    helper.click.assert_called_once_with(185, 270)
                    helper.replace_text.assert_called_once_with("synthetic path")
                else:
                    self.assertIn(unittest.mock.call("xdotool", "mousemove", "--window",
                        window.window, 85, 220, "click", 1), run.call_args_list)

    def test_windows_spaced_token_input_preserves_the_explicit_interior_offset(self):
        with tempfile.TemporaryDirectory() as temporary:
            driver = desktop.Driver.__new__(desktop.Driver)
            driver.profile = Path(temporary)
            driver.session = "windows"
            driver.large_size = (1044, 788)
            driver.executable = Path("owned-buddy.exe")
            driver.config = driver.profile / "config.json"
            driver.backend = Mock(url="http://127.0.0.1:12345")
            driver.checkout = driver.profile
            with patch.object(desktop, "run") as execute:
                driver.drive("token", [("input", "Todoist API token", "public synthetic token", {"labelBottomOffset": 32})])
                for index, bad in enumerate((True, 19, 41, 32.0, "32")):
                    with self.subTest(offset=bad), self.assertRaisesRegex(RuntimeError, "bounded label-to-editor"):
                        driver.drive("invalid-" + str(index), [("input", "Todoist API token", "public synthetic token", {"labelBottomOffset": bad})])
                self.assertEqual(execute.call_count, 1, "Invalid offsets must fail before launching the driver")
            arguments = execute.call_args.args
            commands = arguments[arguments.index("-Actions") + 1].split(",")
            expected = "input64:" + desktop.b64("Todoist API token") + ":" + desktop.b64("public synthetic token") + ":32"
            self.assertIn(expected, commands)

    def test_windows_input_does_not_receive_the_adjacent_unix_row_control(self):
        with tempfile.TemporaryDirectory() as temporary:
            driver = desktop.Driver.__new__(desktop.Driver)
            driver.profile = Path(temporary)
            driver.session = "windows"
            driver.large_size = (1044, 788)
            driver.executable = Path("owned-buddy.exe")
            driver.config = driver.profile / "config.json"
            driver.backend = Mock(url="http://127.0.0.1:12345")
            driver.checkout = driver.profile
            with patch.object(desktop, "run") as run:
                driver.drive("sessions", [("input", "VS Code User directory", "synthetic path", "Scan folder")])
            arguments = run.call_args.args
            commands = arguments[arguments.index("-Actions") + 1].split(",")
            self.assertIn("input64:" + desktop.b64("VS Code User directory") + ":" + desktop.b64("synthetic path"), commands)


class MacInputOwnership(unittest.TestCase):
    def test_normal_environment_cannot_load_input_frameworks(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "", "BUDDY_SETTINGS_FIXTURE": ""}):
            with self.assertRaisesRegex(RuntimeError, "disposable CI fixture"):
                desktop.OwnedMacInput(Mock(pid=123))

    def test_exited_owned_app_cannot_receive_input(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1"}):
            with self.assertRaisesRegex(RuntimeError, "live owned app"):
                desktop.OwnedMacInput(Mock(pid=123, poll=Mock(return_value=0)))

    def test_another_frontmost_app_is_rejected(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123)
        subprocess_module = desktop.OwnedMacInput.require_focus.__globals__["subprocess"]
        with patch.object(subprocess_module, "run", return_value=Mock(stdout="456\n")):
            with self.assertRaisesRegex(RuntimeError, "must be frontmost"):
                helper.require_focus()

    def test_text_replacement_never_types_into_another_frontmost_app(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123)
        subprocess_module = desktop.OwnedMacInput.require_focus.__globals__["subprocess"]
        for text in ("owned fixture text", "owned Unicode 資料"):
            with patch.object(subprocess_module, "run", return_value=Mock(stdout="456\n")) as run:
                with self.assertRaisesRegex(RuntimeError, "must be frontmost"):
                    helper.replace_text(text)
                self.assertEqual(run.call_count, 1)  # Only the focus query ran.

    def test_focus_diagnostics_never_read_another_frontmost_apps_ui(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123)
        subprocess_module = desktop.OwnedMacInput.require_focus.__globals__["subprocess"]
        with patch.object(subprocess_module, "run", return_value=Mock(stdout="456\n")) as run:
            with self.assertRaisesRegex(RuntimeError, "must be frontmost"):
                helper.focus_state()
            self.assertEqual(run.call_count, 1)


class MacInputDelivery(unittest.TestCase):
    def helper(self):
        helper = desktop.OwnedMacInput.__new__(desktop.OwnedMacInput)
        helper.process = Mock(pid=123, poll=Mock(return_value=None))
        helper.graphics, helper.core = Mock(), Mock()
        helper.require_focus = Mock()
        return helper

    def test_click_releases_inherited_modifiers_and_marks_a_single_click(self):
        helper = self.helper()
        helper.graphics.CGEventSourceFlagsState.side_effect = [1 << 20, 0]
        helper.graphics.CGEventSourceButtonState.side_effect = [False, True, False]
        helper.click(100, 200)
        self.assertEqual(helper.last_click_trace, [dict(eventType=5, leftButtonDown=False),
            dict(eventType=1, leftButtonDown=True), dict(eventType=2, leftButtonDown=False)])
        self.assertEqual(helper.graphics.CGEventCreateMouseEvent.call_count, 3)
        self.assertEqual(helper.graphics.CGEventCreateKeyboardEvent.call_count, 8)
        self.assertEqual(helper.graphics.CGEventSetIntegerValueField.call_count, 2)
        for call in helper.graphics.CGEventSetIntegerValueField.call_args_list:
            self.assertEqual(call.args[1:], (1, 1))

    def test_unavailable_button_telemetry_does_not_omit_mouse_up_or_invent_state(self):
        helper = self.helper()
        helper.graphics.CGEventSourceFlagsState.return_value = 0
        helper.graphics.CGEventSourceButtonState.side_effect = [False, RuntimeError("Unavailable telemetry"), False]
        helper.click(100, 200)
        self.assertEqual(helper.graphics.CGEventCreateMouseEvent.call_count, 3)
        self.assertEqual(helper.last_click_trace, [dict(eventType=5, leftButtonDown=False),
            dict(eventType=1, leftButtonDown=None), dict(eventType=2, leftButtonDown=False)])

    def test_stuck_modifiers_fail_before_any_mouse_event(self):
        helper = self.helper()
        helper.graphics.CGEventSourceFlagsState.return_value = 1 << 20
        with self.assertRaisesRegex(RuntimeError, "modifier"):
            helper.click(100, 200)
        helper.graphics.CGEventCreateMouseEvent.assert_not_called()

    def test_denied_post_access_releases_the_event_without_injecting_it(self):
        helper = self.helper()
        helper.graphics.CGPreflightPostEventAccess.return_value = False
        with self.assertRaisesRegex(RuntimeError, "cannot post input"):
            helper.post(1234)
        helper.graphics.CGEventPost.assert_not_called()
        helper.core.CFRelease.assert_called_once_with(1234)

    def test_locator_waits_for_the_feedback_row_to_stop_moving(self):
        driver = desktop.Driver.__new__(desktop.Driver)
        driver.session = "macos"
        old = dict(left=72, top=248, width=100, height=11, save=dict(left=1133, top=282, width=32, height=11))
        new = dict(old, top=290, save=dict(old["save"], top=324))
        window, image = Mock(), Mock()
        with patch.object(desktop.ocr, "locate_image", side_effect=[old, new, new]):
            self.assertEqual(driver.locate(window, image, "field", "Notification audio file"), new)
        self.assertEqual(window.screenshot.call_count, 3)

    def test_a_continuously_moving_target_is_never_clicked(self):
        driver = desktop.Driver.__new__(desktop.Driver)
        driver.session = "macos"
        window, image = Mock(), Mock()
        with patch.object(desktop.ocr, "locate_image", side_effect=[dict(top=i) for i in range(4)]):
            with self.assertRaisesRegex(RuntimeError, "stable"):
                driver.locate(window, image, "field", "Notification audio file")


class UnicodeClipboardOwnership(unittest.TestCase):
    def test_missing_xclip_is_reported_before_clipboard_process_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.sys, "platform", "linux"), \
                patch("shutil.which", return_value=None), \
                patch.object(desktop.subprocess, "Popen", side_effect=FileNotFoundError("xclip")) as launch:
            with self.assertRaisesRegex(RuntimeError, "requires xclip"):
                with desktop.unicode_clipboard("資料"):
                    self.fail("A missing clipboard tool must be rejected before input")
            launch.assert_not_called()

    def test_non_runner_is_refused_before_clipboard_or_process_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}), patch.object(desktop.subprocess, "Popen") as launch:
            with self.assertRaisesRegex(RuntimeError, "disposable Linux"):
                with desktop.unicode_clipboard("資料"):
                    self.fail("An unowned clipboard must not be used")
            launch.assert_not_called()

    def test_clipboard_owner_is_reaped_after_the_native_paste_raises(self):
        owner = Mock()
        owner.poll.return_value = None
        payload = "workspace with spaces 資料".encode("utf-8")
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.sys, "platform", "linux"), \
                patch.object(desktop.shutil, "which", return_value="/fixture/xclip"), \
                patch.object(desktop.subprocess, "Popen", return_value=owner), \
                patch.object(desktop.subprocess, "run", return_value=Mock(returncode=0, stdout=payload)):
            with self.assertRaisesRegex(ValueError, "native paste failure"):
                with desktop.unicode_clipboard(payload.decode("utf-8")):
                    raise ValueError("native paste failure")
        owner.terminate.assert_called_once()
        owner.wait.assert_called_once_with(timeout=3)
        owner.stderr.close.assert_called_once()

    def test_an_old_clipboard_cannot_authorize_pasting_different_text(self):
        owner = Mock()
        owner.poll.return_value = None
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.sys, "platform", "linux"), \
                patch.object(desktop.shutil, "which", return_value="/fixture/xclip"), \
                patch.object(desktop.subprocess, "Popen", return_value=owner), \
                patch.object(desktop.subprocess, "run", return_value=Mock(returncode=0, stdout=b"old data")), \
                patch.object(desktop.time, "monotonic", side_effect=[0, 1, 6]), \
                patch.object(desktop.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "exact Unicode"):
                with desktop.unicode_clipboard("資料"):
                    self.fail("A mismatched clipboard must never reach paste")
        owner.terminate.assert_called_once()
        owner.wait.assert_called_once_with(timeout=3)


class WindowsScrollAnchors(unittest.TestCase):
    def test_scroll_wait_preserves_its_measured_content_anchor_for_the_owned_window(self):
        with tempfile.TemporaryDirectory(prefix="buddy-scroll-command-") as root:
            directory = Path(root)
            driver = object.__new__(desktop.Driver)
            driver.profile, driver.session, driver.large_size = directory, "windows", (1200, 820)
            driver.executable, driver.config, driver.checkout = directory / "owned.exe", directory / "config.json", directory
            driver.backend = Mock(url="http://127.0.0.1:9")
            target, anchor = "Synthetic upcoming task", "Synthetic today task"
            with patch.object(desktop, "run") as run:
                driver.drive("upcoming", [("wait_scroll_text", target, anchor)])
            arguments = list(run.call_args.args)
            commands = arguments[arguments.index("-Actions") + 1].split(",")
            encoded = [base64.b64encode(value.encode()).decode() for value in (target, anchor)]
            self.assertIn("waitscrolltext64:" + ":".join(encoded), commands)

    @staticmethod
    def wait_block():
        lines = Path(__file__).with_name("windows-actions.ps1").read_text(encoding="utf-8").splitlines()
        start = next(index for index, line in enumerate(lines) if "'waitscrolltext64'" in line and "$_ -in" in line)
        depth, block = 0, []
        for line in lines[start:]:
            block.append(line)
            depth += line.count("{") - line.count("}")
            if depth == 0:
                break
        # The first line is `{ <case condition> } {`; keep only the action body.
        body = "\n".join([block[0][block[0].rindex("{") + 1:]] + block[1:])
        return body[:body.rindex("}")]

    def run_wait(self, visible_after, anchor_visible):
        """Run the real wait case against stub locators; return its call journal."""
        stubs = (
            "$journal = [Collections.Generic.List[string]]::new(); $script:probes = 0\n"
            "function Find-OwnedLabelPoint([string] $Encoded, [string] $Mode) {\n"
            "  $text = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($Encoded))\n"
            "  $journal.Add('find:' + $text)\n"
            "  if ($text -eq 'Load date range') {\n"
            "    if ($" + str(anchor_visible).lower() + ") { return @(3, 4) }\n"
            "    throw 'anchor scrolled away'\n"
            "  }\n"
            "  $script:probes++; if ($script:probes -le " + str(visible_after) + ") { throw 'not yet visible' }\n"
            "  return @(1, 2)\n"
            "}\n"
            "function Scroll-OwnedWindow($Delta, $Point) { $journal.Add('scroll') }\n"
            "$parts = @('waitscrolltext64', '" + base64.b64encode(b"Next rows").decode() + "', '"
            + base64.b64encode(b"Load date range").decode() + "')\n"
            "try {\n" + self.wait_block() + "\n} catch { $journal.Add('error:' + $_) }\n"
            "$journal -join '|'\n")
        with tempfile.TemporaryDirectory(prefix="buddy-wait-anchor-") as root:
            script = Path(root) / "wait.ps1"
            script.write_text(stubs, encoding="utf-8")
            result = subprocess.run(["pwsh", "-NoProfile", "-File", str(script)], capture_output=True, text=True,
                encoding="utf-8", timeout=60, check=True)
        return result.stdout.strip().split("|")

    @unittest.skipUnless(shutil.which("pwsh"), "PowerShell is required to execute the Windows action case")
    def test_visible_target_never_requires_its_scroll_anchor(self):
        # Native run 39128144643: an explicit wheel revealed Next rows and
        # moved Load date range above the viewport. The eager anchor lookup
        # failed although the requested target was already visible.
        self.assertEqual(self.run_wait(0, anchor_visible=False), ["find:Next rows"])

    @unittest.skipUnless(shutil.which("pwsh"), "PowerShell is required to execute the Windows action case")
    def test_hidden_target_measures_its_anchor_once_before_scrolling(self):
        self.assertEqual(self.run_wait(2, anchor_visible=True), ["find:Next rows", "find:Load date range", "scroll",
            "find:Next rows", "scroll", "find:Next rows"])
        self.assertEqual(self.run_wait(1, anchor_visible=False),
            ["find:Next rows", "find:Load date range", "error:anchor scrolled away"])


if __name__ == "__main__":
    unittest.main()

class DiagnosticsEnvironment(unittest.TestCase):
    def test_unix_launch_preserves_caller_trace_filters_and_keeps_secrets_scrubbed(self):
        # Every process/window operation is mocked. The fixture flag exercises
        # only environment construction and never authorizes a real desktop.
        caller = "warn,reqwest=trace,hyper=trace,github_copilot_sdk=trace"
        with tempfile.TemporaryDirectory(prefix="buddy-diagnostic-env-") as temporary:
            directory = Path(temporary)
            driver = object.__new__(desktop.Driver)
            driver.profile, driver.session, driver.large_size = directory, "x11", (1200, 820)
            driver.executable, driver.config = directory / "unused-owned.exe", directory / "config.json"
            driver.backend = Mock(url="http://127.0.0.1:9")
            process = Mock(pid=123, poll=Mock(return_value=None), wait=Mock(return_value=0))
            window = Mock(ready=Mock(return_value=True))
            with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "RUST_LOG": caller,
                    "GH_TOKEN": "synthetic-gh-token", "GITHUB_TOKEN": "synthetic-github-token", "ELECTRON_RUN_AS_NODE": "1"}), \
                    patch.object(desktop.subprocess, "Popen", return_value=process) as launch, \
                    patch.object(desktop, "UnixWindow", return_value=window), \
                    patch.object(desktop, "run"), patch.object(desktop.time, "sleep"):
                driver.unix(directory, [], concurrent=False, record=False)
                observed = launch.call_args.kwargs["env"]
                self.assertEqual(launch.call_count, 1)
                self.assertIn(caller, observed["RUST_LOG"])
                self.assertIn("buddy::settings_view=debug", observed["RUST_LOG"])
                self.assertEqual(os.environ["RUST_LOG"], caller)
                for key in ("GH_TOKEN", "GITHUB_TOKEN", "ELECTRON_RUN_AS_NODE"):
                    self.assertNotIn(key, observed)
