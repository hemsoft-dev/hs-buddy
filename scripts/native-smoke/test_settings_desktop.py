"""Choose supported fixtures before requesting sizes; never accept clamping."""
import importlib.util
import json
import os
from pathlib import Path
import struct
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
            for point, expected, position in [((95, 360), -1, (105, 380)), (None, -10, (460, 345))]:
                owned.graphics.reset_mock()
                driver.scroll(window, -120, point)
                self.assertEqual(owned.graphics.CGEventCreateScrollWheelEvent.call_args.args, (None, 1, 1, expected))
                observed = owned.graphics.CGEventSetLocation.call_args.args[1]
                self.assertEqual((observed.x, observed.y), position)

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
                    ("wait_text", "Saved to results"), ("key", "Return")])
            args = execute.call_args.args
            commands = args[args.index("-Actions") + 1].split(",")
            self.assertEqual(commands[1], "waitmodeltext64:" + desktop.b64("Synthetic offline model"))
            self.assertEqual(commands[2], "waittext64:" + desktop.b64("Saved to results"))
            self.assertEqual(commands[3], "key:13")
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
        helper.click(100, 200)
        self.assertEqual(helper.graphics.CGEventCreateKeyboardEvent.call_count, 8)
        self.assertEqual(helper.graphics.CGEventSetIntegerValueField.call_count, 2)
        for call in helper.graphics.CGEventSetIntegerValueField.call_args_list:
            self.assertEqual(call.args[1:], (1, 1))

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


if __name__ == "__main__":
    unittest.main()
