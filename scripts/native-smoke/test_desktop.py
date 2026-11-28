"""The visual gate rejects a blank window and a title/tab-only rendering."""

import unittest
import json
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event
from unittest.mock import patch
from desktop import FINANCE_FIXTURE, OFFLINE_PROXY, PROXY_VARIABLES, cached_finance, fixture, inspect_screenshot, offline_network, require_cached_finance, seed_finance_cache, owned_wayland_node, recording, require_dashboard, require_dpi_sequence, require_size, run, wayland_keyboard

BODY = "Your Universal Productivity Companion Customize Command Center Configure Accounts"


class DashboardGate(unittest.TestCase):
    def test_windows_console_helpers_have_no_window_even_with_captured_output(self):
        import desktop
        with patch.object(desktop.os, "name", "nt"), \
                patch.object(desktop.subprocess, "CREATE_NO_WINDOW", 0x08000000, create=True), \
                patch.object(desktop.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "owned output", "")) as child:
            self.assertEqual(run("pwsh", "-NoProfile", "-File", "owned.ps1", creationflags=0x200), "owned output")
        self.assertEqual(child.call_args.kwargs["creationflags"], 0x08000200)
        self.assertTrue(child.call_args.kwargs["capture_output"])

    def test_an_x11_shortcut_leaves_the_pointer_where_it_is(self):
        # #880: a page hands Buddy's shortcuts back, so the driver no longer
        # parks the pointer off the page before one, as #875's driver did.
        import desktop
        window = desktop.UnixWindow(None, "x11")
        window.window = "42"
        with patch.object(desktop, "run") as run_command, patch.object(desktop.time, "sleep"):
            window.key("q")
            shortcut = [call.args for call in run_command.call_args_list]
        self.assertFalse(any("mousemove" in args for args in shortcut))
        self.assertEqual(shortcut[-1], ("xdotool", "key", "--clearmodifiers", "ctrl+q"))

    def test_alt_keys_reach_buddy_on_each_unix_session(self):
        import desktop
        geometry = "\n".join(["WINDOW=42", "X=80", "Y=40", "WIDTH=1200", "HEIGHT=820", "SCREEN=0"])
        window = desktop.UnixWindow(None, "x11")
        window.window = "42"
        with patch.object(desktop, "run", return_value=geometry) as run_command, patch.object(desktop.time, "sleep"):
            window.alt_key("=")
            calls = [call.args for call in run_command.call_args_list]
        # A page keeps Alt keys, so the pointer leaves it for the title bar first.
        self.assertEqual(calls[1], ("xdotool", "mousemove", "--window", "42", 600, 15))
        self.assertEqual(calls[-1], ("xdotool", "key", "--clearmodifiers", "alt+equal"))
        with patch.object(desktop, "run", return_value=geometry) as run_command, patch.object(desktop.time, "sleep"):
            window.alt_key("-")
        self.assertEqual(run_command.call_args_list[-1].args, ("xdotool", "key", "--clearmodifiers", "alt+minus"))

        mac = desktop.UnixWindow(None, "macos")
        mac.process = type("Process", (), {"pid": 7})()
        with patch.object(desktop, "run") as run_command, patch.object(desktop.time, "sleep"):
            mac.alt_key("=")
            mac.alt_key("-")
        scripts = [call.args[2] for call in run_command.call_args_list]
        self.assertIn("key code 24 using option down", scripts[0])
        self.assertIn("key code 27 using option down", scripts[1])
        self.assertTrue(all("unix id is 7" in script for script in scripts))

        wayland = desktop.UnixWindow(None, "wayland")
        wayland.window = "9"
        with patch.object(desktop, "run") as run_command, patch.object(desktop.time, "sleep"):
            wayland.alt_key("=")
        self.assertEqual(run_command.call_args_list[-1].args, ("wtype", "-M", "alt", "-k", "equal", "-m", "alt"))
        with self.assertRaisesRegex(RuntimeError, "Unsupported Alt key"):
            window.alt_key("0")

    def test_hover_rests_the_pointer_in_the_middle_of_the_window(self):
        import desktop
        window = desktop.UnixWindow(None, "x11")
        window.window = "42"
        geometry = "\n".join(["WINDOW=42", "X=80", "Y=40", "WIDTH=1200", "HEIGHT=820", "SCREEN=0"])
        with patch.object(desktop, "run", return_value=geometry) as run_command, patch.object(desktop.time, "sleep"):
            window.hover()
        self.assertEqual(run_command.call_args_list[-1].args, ("xdotool", "mousemove", "--window", "42", 600, 410))
        with patch.object(desktop, "run", return_value=geometry) as run_command, patch.object(desktop.time, "sleep"):
            window.hover("title")
        self.assertEqual(run_command.call_args_list[-1].args, ("xdotool", "mousemove", "--window", "42", 600, 15))
        with patch.object(desktop, "run", return_value=geometry), self.assertRaises(RuntimeError):
            window.hover("sidebar")
        with self.assertRaises(RuntimeError):
            desktop.UnixWindow(None, "wayland").hover()

    def test_seat_keyboard_lives_through_actions_and_is_reaped_on_failure(self):
        spawn = subprocess.Popen
        children = []
        def owned_child(*args, **kwargs):
            child = spawn([sys.executable, "-c", "import time; time.sleep(300)"],
                creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0, **kwargs)
            children.append(child)
            return child
        primary = ValueError("Owned app action failed")
        with TemporaryDirectory() as temporary, patch("desktop.subprocess.Popen", side_effect=owned_child):
            with self.assertRaises(ValueError) as observed:
                with wayland_keyboard("wayland", Path(temporary)):
                    self.assertEqual(len(children), 1)
                    self.assertIsNone(children[0].poll())
                    raise primary
            self.assertIs(observed.exception, primary)
            self.assertEqual(len(children), 1, "Do not replay a failed app run")
            self.assertIsNotNone(children[0].poll(), "Owned keyboard must not outlive its app")

    def test_both_dpi_transitions_need_measured_bounds(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaises(FileNotFoundError):
                require_dpi_sequence(directory)
            def write(dpi, width, height):
                (directory / f"dpi-{dpi}.json").write_text(json.dumps(dict(
                    width=width, height=height, simulatedDpi=dpi, actualMonitorDpi=96)))
            write(120, 816, 608)  # A successfully delivered but ignored event.
            write(96, 816, 608)
            with self.assertRaises(RuntimeError):
                require_dpi_sequence(directory)
            write(120, 1044, 788)  # The small runner desktop's actual clamp.
            with self.assertRaises(RuntimeError):
                require_dpi_sequence(directory)
            write(120, 1020, 760)
            write(96, 1020, 760)  # Scale-up alone cannot qualify a return event.
            with self.assertRaises(RuntimeError):
                require_dpi_sequence(directory)
            write(96, 816, 608)
            require_dpi_sequence(directory)

    def test_menu_requires_its_own_visible_content_and_exact_version(self):
        # Dashboard/card rendering is asserted on the unobstructed captures.
        # Opening a menu must additionally render Exit, not merely its trigger.
        for text, passes in (("Buddy File Exit Version 0.1.2", True),
                             (f"{BODY} File 0.1.2", False),
                             ("Buddy File Exit Version 0.1.20", False),
                             ("Buddy Exit Version 0.1.2", False)):
            with self.subTest(text=text), TemporaryDirectory() as temporary, patch("desktop.run", return_value=text):
                path = Path(temporary) / "keyboard-menu.png"
                if passes:
                    inspect_screenshot(path, "0.1.2", menu=True)
                else:
                    with self.assertRaises(RuntimeError):
                        inspect_screenshot(path, "0.1.2", menu=True)

    def test_partial_ocr_results_cannot_be_combined_to_qualify(self):
        with TemporaryDirectory() as temporary, patch("desktop.run", side_effect=[
                f"{BODY} 0.1.20", "Version 0.1.2", BODY, "", f"{BODY} 0.1.20"]) as recognize:
            with self.assertRaises(RuntimeError):
                inspect_screenshot(Path(temporary) / "dashboard.png", "0.1.2")
            self.assertEqual(recognize.call_count, 5)

    def test_scaled_analysis_still_requires_exact_version_and_every_marker_in_one_result(self):
        with TemporaryDirectory() as temporary, patch("desktop.run", side_effect=[
                f"{BODY} 0.1.2.", "Version 0.1.2", BODY, "", f"{BODY} 0.1.2"]) as recognize:
            path = Path(temporary) / "dashboard.png"
            path.write_bytes(b"original window pixels")
            inspect_screenshot(path, "0.1.2")
            self.assertEqual(recognize.call_count, 5)
            self.assertEqual(path.read_bytes(), b"original window pixels")
            self.assertEqual(json.loads(path.with_suffix(".ocr-mode.json").read_text()), dict(mode=11, scale=3))
            scale = recognize.call_args_list[3].args
            self.assertEqual(scale[0], "ffmpeg")
            self.assertIn("scale=iw*3:ih*3:flags=neighbor", scale)
            self.assertEqual(recognize.call_args_list[4].args[1], path.with_suffix(".ocr-analysis-3x.png"))

    def test_scaled_analysis_does_not_repair_misread_digits_or_accept_a_longer_version(self):
        for version in ("0.1.20", "0.1.2.1", "0.1.2l", "0.1.2."):
            with self.subTest(version=version), TemporaryDirectory() as temporary, patch("desktop.run", side_effect=[
                    "Version 0.1.2", BODY, BODY, "", f"{BODY} {version}"]):
                with self.assertRaises(RuntimeError):
                    inspect_screenshot(Path(temporary) / "dashboard.png", "0.1.2")

    def test_fallback_requires_a_complete_current_result_and_stops_on_success(self):
        with TemporaryDirectory() as temporary, patch("desktop.run", side_effect=[
                f"{BODY} 0.1.20", f"{BODY} 0.1.2"]) as recognize:
            path = Path(temporary) / "dashboard.png"
            inspect_screenshot(path, "0.1.2")
            self.assertEqual(recognize.call_count, 2)
            self.assertEqual(json.loads(path.with_suffix(".ocr-mode.json").read_text())["mode"], 3)
            self.assertEqual(path.with_suffix(".ocr.txt").read_text(), f"{BODY} 0.1.2")

    def test_ignored_or_clamped_resize_fails(self):
        for actual in (dict(width=1200, height=820), dict(width=900, height=600),
                       dict(width=float("nan"), height=650), dict(width=900, height=float("inf"))):
            with self.assertRaises(RuntimeError):
                require_size(actual, 900, 650)
        require_size(dict(width=900, height=650), 900, 650)

    def test_wayland_capture_cannot_select_another_process_or_window(self):
        ours = dict(pid=42, app_id="buddy-native", id=7, rect=dict(x=80, y=90, width=900, height=650))
        other = dict(pid=43, app_id="buddy-native", id=8)
        tree = dict(nodes=[dict(nodes=[other])], floating_nodes=[ours])
        self.assertEqual(owned_wayland_node(tree, 42, 7)["rect"], dict(x=80, y=90, width=900, height=650))
        self.assertIsNone(owned_wayland_node(tree, 42, 8))
        self.assertIsNone(owned_wayland_node(tree, 99))

    def test_header_without_the_card_body_cannot_qualify(self):
        for text in ("Your Universal Productivity Companion Customize 0.1.2",
                     "Your Universal Productivity Companion Customize Command Center 0.1.2"):
            with self.assertRaises(RuntimeError):
                require_dashboard(text, "0.1.2")
    def test_command_timeout_can_be_set_by_the_caller(self):
        self.assertEqual(run(sys.executable, "-c", "print('owned child')", timeout=5), "owned child")
        with self.assertRaises(subprocess.TimeoutExpired):
            run(sys.executable, "-c", "import time; time.sleep(10)", timeout=0.1)

    def recording_storage_failure(self, action_error):
        started, release = Event(), Event()
        calls = []
        class StorageFault:
            def screenshot(self, path):
                calls.append(path)
                started.set()
                if not release.wait(10):
                    raise TimeoutError("Storage fixture was not released")
                path.write_bytes(b"owned fault-injection data")
        with TemporaryDirectory() as temporary:
            frames = Path(temporary) / "frames"
            try:
                with recording(StorageFault(), frames):
                    self.assertTrue(started.wait(10))
                    # A real filesystem failure in the pending screenshot write.
                    frames.rmdir()
                    release.set()
                    if action_error:
                        raise action_error
            except BaseException as observed:
                self.assertEqual(len(calls), 1, "A failed capture must not replay")
                return observed
            self.fail("The failed required recording must fail qualification")

    def test_recording_write_failure_keeps_original_action_exception(self):
        primary = ValueError("owned primary action failure")
        self.assertIs(self.recording_storage_failure(primary), primary)

    def test_recording_write_failure_fails_an_otherwise_successful_action(self):
        self.assertIsInstance(self.recording_storage_failure(None), FileNotFoundError)

    def test_theme_fixtures_include_the_persisted_electron_color_presets(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            light = json.loads(fixture(root / "light", "light").read_text())
            dark = json.loads(fixture(root / "dark", "dark").read_text())
            self.assertEqual(light["ui"]["bgPrimary"], "#ffffff")
            self.assertEqual(light["ui"]["fontColor"], "#1f1f1f")
            self.assertEqual(dark["ui"]["bgPrimary"], "#1e1e1e")
            self.assertEqual(dark["ui"]["fontColor"], "#cccccc")
            for config in (light, dark):
                self.assertTrue(config["ui"]["dashboardCards"]["command-center"])
                self.assertEqual(len([key for key in config["ui"] if key.endswith("Color") or key.startswith("bg") or key.startswith("statusBar")]), 6)

    def test_blank_and_shell_only_fail(self):
        for text in ("", "Buddy File Edit View Help Dashboard", "Customize 0.1.1"):
            with self.subTest(text=text), self.assertRaises(RuntimeError):
                require_dashboard(text, "0.1.1")

    def test_stale_version_fails(self):
        with self.assertRaises(RuntimeError):
            require_dashboard(f"{BODY} 0.1.1", "0.1.2")

    def test_a_version_prefix_cannot_qualify_a_different_release(self):
        for observed in ("0.1.20", "10.1.2", "0.1.2.1"):
            with self.subTest(observed=observed), self.assertRaises(RuntimeError):
                require_dashboard(f"{BODY} {observed}", "0.1.2")

    def test_body_and_current_version_pass(self):
        require_dashboard(f"{BODY} Version 0.1.2", "0.1.2")


class CachedFinance(unittest.TestCase):
    def test_the_seed_shows_only_finance_with_a_fresh_cache_for_its_watchlist(self):
        with TemporaryDirectory() as temporary:
            config = fixture(Path(temporary) / "dark", "dark")
            cache = seed_finance_cache(config, now_ms=1_764_000_000_000)
            settings = json.loads(config.read_text())
            self.assertTrue(settings["ui"]["dashboardCards"]["finance"])
            self.assertFalse(settings["ui"]["dashboardCards"]["command-center"])
            self.assertEqual(settings["finance"]["watchlist"], [FINANCE_FIXTURE["symbol"]])
            self.assertEqual(settings["packageQualificationSentinel"], {"preserve": True})
            self.assertEqual(cache, config.with_name("finance-cache.json"))
            document = json.loads(cache.read_text())
            self.assertEqual(document["version"], 3)
            self.assertEqual(document["timestamp"], 1_764_000_000_000)
            self.assertEqual(document["symbols"], [FINANCE_FIXTURE["symbol"]])
            [quote] = document["quotes"]
            self.assertEqual(quote["symbol"], FINANCE_FIXTURE["symbol"])
            self.assertEqual(quote["price"], 987.65)
            self.assertEqual(set(quote), {"symbol", "name", "price", "change", "changePercent", "previousClose", "marketOpen"})

    def test_the_relaunch_is_offline_and_the_environment_is_restored(self):
        import os
        touched = PROXY_VARIABLES + ("NO_PROXY", "no_proxy")
        # Whatever the host has is replaced for the test: one variable set,
        # one exclusion set, every other proxy variable absent.
        with patch.dict("os.environ", {}, clear=False):
            for name in touched:
                os.environ.pop(name, None)
            os.environ["HTTPS_PROXY"] = "http://proxy.example:8080"
            os.environ["NO_PROXY"] = "localhost"
            before = {name: os.environ.get(name) for name in touched}
            with offline_network():
                for name in PROXY_VARIABLES:
                    self.assertEqual(os.environ[name], OFFLINE_PROXY)
                self.assertNotIn("NO_PROXY", os.environ)
                self.assertNotIn("no_proxy", os.environ)
            self.assertEqual({name: os.environ.get(name) for name in touched}, before)

    def test_the_capture_needs_the_cached_name_price_and_version(self):
        require_cached_finance("Finance Qualification Quote OPEN 987.65 +1.25 (+0.13%) 0.1.2", "0.1.2")
        require_cached_finance("Finance Qualification Quote 987,65 0.1.2", "0.1.2")
        for text in (
            "Finance Qualification Quote 0.1.2",
            "Finance Qualification Quote 1987.65 0.1.2",
            "Finance Qualification Quote 987.655 0.1.2",
            "Finance 987.65 0.1.2",
            "Qualification Quote 987.65 0.1.2",
            "Finance Qualification Quote 987.65 0.1.1",
        ):
            with self.subTest(text=text), self.assertRaises(RuntimeError):
                require_cached_finance(text, "0.1.2")

    def test_a_refresh_that_rewrote_the_cache_fails_the_step(self):
        def drive(executable, directory, config, restored=False):
            self.assertTrue(restored)
            import os
            self.assertEqual(os.environ["HTTPS_PROXY"], OFFLINE_PROXY)
            (directory / "dashboard.png").write_bytes(b"png")
            if rewrite:
                config.with_name("finance-cache.json").write_text("{}")
        for rewrite in (False, True):
            with self.subTest(rewrite=rewrite), TemporaryDirectory() as temporary:
                directory = Path(temporary) / "dark"
                config = fixture(directory, "dark")
                with patch("desktop.inspect_screenshot") as inspect:
                    if rewrite:
                        with self.assertRaisesRegex(RuntimeError, "rewrote the cached quotes"):
                            cached_finance("buddy", directory, config, drive, "0.1.2")
                    else:
                        cached_finance("buddy", directory, config, drive, "0.1.2")
                capture = directory / "finance-cached" / "finance-cached-dark.png"
                self.assertTrue(capture.exists())
                inspect.assert_called_once_with(capture, "0.1.2", require=require_cached_finance)


if __name__ == "__main__":
    unittest.main()
