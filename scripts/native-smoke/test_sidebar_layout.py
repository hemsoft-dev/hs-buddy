"""The installed sidebar-width, divider-drag and ui.zoomLevel step."""
import importlib.util
import json
import struct
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, call, patch

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


layout = load("sidebar-layout")
github = load("github-desktop")
settings = github.settings
mac_input = load("macos-input")

# The dark and light colours of the actual Ubuntu GitHub captures
# (review-dark/list-dark.png, accounts-light/second-account-light.png): the
# activity bar, the sidebar and workspace background, and the divider line.
DARK = dict(bar=(51, 51, 51), background=(30, 30, 30), line=(60, 60, 60))
LIGHT = dict(bar=(44, 44, 44), background=(255, 255, 255), line=(229, 229, 229))


def row(width, bar, divider, colours=DARK, line_width=1, border=False, icon=None):
    """One capture row; `icon` is (first, last) columns an activity bar glyph
    paints in it."""
    pixels = [colours["bar"]] * bar + [colours["background"]] * (width - bar)
    for x in range(divider, divider + line_width):
        pixels[x] = colours["line"]
    if border:
        pixels[0] = (128, 128, 128)
    if icon:
        for x in range(icon[0], icon[1] + 1):
            pixels[x] = (204, 204, 204)
    return pixels


def png(path, width, height):
    """A file with a PNG signature and IHDR size, all the analysis reads."""
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + b"\0\0\0\rIHDR" + struct.pack(">II", width, height) + b"\0" * 16)
    return path


def ffmpeg_rows(plan):
    """A `run` that answers the analysis's ffmpeg row reads from `plan`:
    capture name -> (bar, divider)."""
    def run(*args, **kwargs):
        if args[0] != "ffmpeg":
            return ""
        image, raw = Path(args[args.index("-i") + 1]), Path(args[-1])
        width = struct.unpack(">II", image.read_bytes()[16:24])[0]
        bar, divider = plan[image.name]
        raw.write_bytes(b"".join(bytes(pixel) for pixel in row(width, bar, divider)))
        return ""
    return run


class RowGeometry(unittest.TestCase):
    def test_the_recorded_captures_give_the_bar_edge_and_the_divider_centre(self):
        for colours in (DARK, LIGHT):
            with self.subTest(colours=colours):
                self.assertEqual(layout.row_split(row(1200, 48, 257, colours)), (48, 257.5))

    def test_a_window_border_and_a_hidpi_line_are_measured_as_one_feature(self):
        self.assertEqual(layout.row_split(row(2400, 96, 600, line_width=2, border=True)), (96, 601))

    def test_a_row_without_a_divider_fails(self):
        with self.assertRaisesRegex(RuntimeError, "no activity bar edge and sidebar divider"):
            layout.row_split([DARK["bar"]] * 48 + [DARK["background"]] * 1152)

    def test_every_sampled_row_must_agree(self):
        geometry = layout.split_geometry([row(1200, 48, 480), row(1200, 48, 481)], 1200)
        self.assertEqual((geometry["bar"], geometry["divider"]), (48, 480.5))
        self.assertAlmostEqual(geometry["share"], (480.5 - 48) / 1152)
        with self.assertRaisesRegex(RuntimeError, "disagree"):
            layout.split_geometry([row(1200, 48, 480), row(1200, 48, 520)], 1200)

    def test_an_activity_bar_icon_on_some_sampled_rows_is_not_the_bar_edge(self):
        # The Ubuntu zoom-125-dark capture of run 45523868491: at 125% the
        # bar is 60px and two sampled rows cross icons (glyphs at 26-33 and
        # a stroke at 23-29 and 33-39), the third row does not.
        rows = [row(1200, 60, 345, line_width=2, icon=(26, 33)), row(1200, 60, 345, line_width=2),
            row(1200, 60, 345, line_width=2, icon=(23, 39))]
        geometry = layout.split_geometry(rows, 1200)
        self.assertEqual((geometry["bar"], geometry["divider"]), (60, 346))
        # A stroke as thin as a line, on one row only, is left out too.
        rows[0] = row(1200, 60, 345, line_width=2, icon=(30, 31))
        self.assertEqual(layout.split_geometry(rows, 1200)["bar"], 60)
        # A row that shows no divider at all still fails.
        with self.assertRaisesRegex(RuntimeError, "no activity bar edge"):
            layout.split_geometry([rows[1], [DARK["bar"]] * 60 + [DARK["background"]] * 1140], 1200)

    def test_the_share_must_be_within_the_slack(self):
        wide = dict(width=1200, bar=48)
        layout.require_share(dict(wide, share=0.42), 0.4, "launched")
        with self.assertRaisesRegex(RuntimeError, "launched sidebar takes 0.450 of the row, expected 0.400"):
            layout.require_share(dict(wide, share=0.45), 0.4, "launched")

    def test_the_expected_share_follows_the_native_minimums_at_any_scale(self):
        # 1,200px at 100%: the saved share holds.
        self.assertAlmostEqual(layout.expected_share(0.25, dict(width=1200, bar=48)), 0.25)
        # 1,200 physical pixels at 120 DPI and 125%: 72px per 48, so the row
        # is 752 at 100% and a quarter (188) is held at the 200 minimum.
        self.assertAlmostEqual(layout.expected_share(0.25, dict(width=1200, bar=72)), 200 / 752)
        # A wide share is held so the content keeps its 400.
        self.assertAlmostEqual(layout.expected_share(0.9, dict(width=1200, bar=48)), 752 / 1152)
        # A row too narrow for both minimums shares them in proportion.
        self.assertAlmostEqual(layout.expected_share(0.4, dict(width=1100, bar=96)), 1 / 3)
        # 1,200 physical pixels at 175% DPI: an 84px bar and a 637.7px row,
        # where a quarter (159) is held at the 200 minimum: about 31%.
        self.assertAlmostEqual(layout.expected_share(0.25, dict(width=1200, bar=84)), 200 / (1116 / 1.75))

    def test_saved_sizes_must_be_positive_widths(self):
        self.assertEqual(layout.saved_share([300, 900, 350]), 0.25)
        for sizes in (None, [300], [300, 0], [300, True], [300, float("nan")], "300,900"):
            with self.subTest(sizes=sizes), self.assertRaises(RuntimeError):
                layout.saved_share(sizes)

    def test_the_drag_goes_from_the_divider_to_the_target_share_on_a_sampled_row(self):
        geometry = dict(width=1200, bar=48, divider=508.5, rows=[688, 713, 738])
        self.assertEqual(layout.drag_points(geometry, 0.25), (508, 713, 336))
        with self.assertRaisesRegex(RuntimeError, "too short"):
            layout.drag_points(dict(geometry, divider=340), 0.25)

    def test_measure_reads_the_capture_and_keeps_a_receipt(self):
        with TemporaryDirectory() as temporary:
            image = png(Path(temporary) / "sidebar-width-dark.png", 1200, 820)
            geometry = layout.measure(image, ffmpeg_rows({image.name: (48, 508)}))
            self.assertEqual(geometry["rows"], [688, 713, 738])
            self.assertEqual(json.loads(image.with_suffix(".split.json").read_text()), geometry)


def fixture_config(directory, ui=None):
    config = directory / "config.json"
    config.write_text(json.dumps(dict(ui=dict(theme="dark", **(ui or {})), native=dict(activeSection="github"))))
    return config


class Qualification(unittest.TestCase):
    """The installed step, with a stand-in driver and the analysis's own reads."""

    # Launched at [480, 720] (a 40% share of 1,152px: 461px), dragged to 25%
    # (288px), and launched at 125% (a 60px bar and 25% of 1,140px).
    PLAN = {"sidebar-width-dark.png": (48, 509), "sidebar-drag-before-dark.png": (48, 509),
        "sidebar-drag-dark.png": (48, 336), "zoom-125-dark.png": (60, 345)}

    def run_step(self, plan=None, drag_saves=(288, 864, 350), ui=None):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        config = fixture_config(root, ui)
        launches = []

        def drive(name, actions, record=False):
            directory = root / name
            directory.mkdir()
            value = json.loads(config.read_text())["ui"]
            launches.append((name, actions, record, dict(value)))
            for kind, *values in actions:
                if kind == "capture":
                    png(directory / (values[0] + ".png"), 1200, 820)
                if kind == "drag" and drag_saves is not None:
                    saved = json.loads(config.read_text())
                    saved["ui"]["paneSizes"] = list(drag_saves)
                    config.write_text(json.dumps(saved))
            return directory

        driver = Mock()
        driver.drive.side_effect = drive
        check = Mock()
        try:
            result = layout.qualify(driver, config, check, ffmpeg_rows(plan or self.PLAN))
        finally:
            self.restored = json.loads(config.read_text())["ui"]
        return result, launches, check

    def test_launch_drag_and_zoom_are_measured_and_the_fixture_is_restored(self):
        result, launches, check = self.run_step(ui=dict(paneSizes=[300, 900], zoomLevel=100))
        self.assertEqual([launch[0] for launch in launches], ["sidebar-width-dark", "sidebar-drag-dark", "zoom-125-dark"])
        self.assertEqual(launches[0][3]["paneSizes"], [480, 720])
        self.assertEqual(launches[0][3]["zoomLevel"], 100)
        self.assertIs(launches[0][3]["assistantOpen"], False)
        # The drag starts on the measured divider and ends at a 25% share,
        # and its launch is recorded.
        drag = [action for action in launches[1][1] if action[0] == "drag"]
        self.assertEqual(drag, [("drag", 510, 713, 336)])
        self.assertTrue(launches[1][2])
        self.assertEqual((launches[2][3]["paneSizes"], launches[2][3]["zoomLevel"]), ([300, 900], 125))
        self.assertEqual([c.args[0].name for c in check.call_args_list], ["sidebar-width-dark.png", "zoom-125-dark.png"])
        self.assertAlmostEqual(result["zoomRatio"], 1.25)
        self.assertEqual(result["savedSizes"], [288, 864, 350])
        self.assertEqual(self.restored, dict(theme="dark", paneSizes=[300, 900], zoomLevel=100))

    def test_missing_keys_stay_missing_after_the_step(self):
        self.run_step()
        self.assertEqual(self.restored, dict(theme="dark"))

    def test_a_launch_at_the_wrong_width_fails_and_still_restores(self):
        with self.assertRaisesRegex(RuntimeError, "launched sidebar"):
            self.run_step(plan=dict(self.PLAN, **{"sidebar-width-dark.png": (48, 336)}),
                ui=dict(paneSizes=[300, 900]))
        self.assertEqual(self.restored, dict(theme="dark", paneSizes=[300, 900]))

    def test_a_drag_that_saved_nothing_fails(self):
        with self.assertRaisesRegex(RuntimeError, "saved ui.paneSizes"):
            self.run_step(drag_saves=None)
        self.assertEqual(self.restored, dict(theme="dark"))

    def test_a_drag_that_moved_the_assistant_entry_fails(self):
        with self.assertRaisesRegex(RuntimeError, "assistant entry"):
            self.run_step(drag_saves=(288, 864, 600))

    def test_a_divider_left_where_it_was_fails(self):
        with self.assertRaisesRegex(RuntimeError, "dragged sidebar"):
            self.run_step(plan=dict(self.PLAN, **{"sidebar-drag-dark.png": (48, 509)}))

    def test_a_relaunch_that_lays_out_differently_fails(self):
        with self.assertRaisesRegex(RuntimeError, "second launch"):
            self.run_step(plan=dict(self.PLAN, **{"sidebar-drag-before-dark.png": (48, 520)}))

    def test_a_drag_held_at_the_sidebar_minimum_saves_the_share_it_shows(self):
        # 1,200 physical pixels at 175% DPI: an 84px bar, a 637.7px row at
        # 100%. The 40% launch is held so the content keeps 400 (37.3%), the
        # 25% drag target at the 200 minimum (31.4%), and the file holds what
        # the divider shows, [376, 824] of the saved 1,200, not a quarter.
        plan = {"sidebar-width-dark.png": (84, 500), "sidebar-drag-before-dark.png": (84, 500),
            "sidebar-drag-dark.png": (84, 434), "zoom-125-dark.png": (105, 470)}
        result, _, _ = self.run_step(plan=plan, drag_saves=(376.3, 823.7, 350))
        self.assertAlmostEqual(result["dragged"]["share"], (434.5 - 84) / 1116)
        self.assertEqual(result["savedSizes"], [376.3, 823.7, 350])
        # A file at the unclamped quarter does not match what is shown.
        with self.assertRaisesRegex(RuntimeError, "not a 0.314 share"):
            self.run_step(plan=plan, drag_saves=(300, 900, 350))

    def test_an_unscaled_zoom_fails(self):
        with self.assertRaisesRegex(RuntimeError, "scaled the activity bar by 1.000"):
            self.run_step(plan=dict(self.PLAN, **{"zoom-125-dark.png": (48, 324)}))


class DragInput(unittest.TestCase):
    def driver(self, root, session):
        driver = object.__new__(settings.Driver)
        driver.session, driver.profile = session, root
        driver.config, driver.executable, driver.checkout = root / "config.json", root / "owned.exe", root
        driver.large_size, driver.backend = (1200, 820), Mock(url="http://127.0.0.1:9")
        return driver

    def test_windows_passes_the_capture_points_to_its_owned_input_helper(self):
        with TemporaryDirectory() as temporary:
            with patch.object(settings, "run") as execute:
                self.driver(Path(temporary), "windows").drive("drag", [("drag", 508, 713, 336)])
            args = execute.call_args.args
            self.assertIn("drag:508:713:336", args[args.index("-Actions") + 1].split(","))

    def test_a_drag_needs_three_whole_points(self):
        for values in ([508, 713], [508.5, 713, 336], [508, -1, 336], [True, 713, 336]):
            with self.subTest(values=values), self.assertRaises(RuntimeError):
                settings.drag_values(values)

    def test_x11_presses_moves_and_always_releases(self):
        driver = self.driver(Path("."), "x11")
        window = Mock(window="owned-window")
        with patch.object(settings, "run") as execute, patch.object(settings.time, "sleep"):
            driver.drag(window, Path("."), [508, 713, 336])
        calls = [c.args for c in execute.call_args_list]
        self.assertEqual(calls[1], ("xdotool", "mousemove", "--window", "owned-window", 508, 713))
        self.assertEqual(calls[2], ("xdotool", "mousedown", 1))
        self.assertEqual(calls[-2], ("xdotool", "mousemove", "--window", "owned-window", 336, 713))
        self.assertEqual(calls[-1], ("xdotool", "mouseup", 1))
        self.assertEqual(len(calls), 3 + settings.DRAG_STEPS + 1)

        def fail(*args, **kwargs):
            if args[1] == "mousemove" and args[-2] != 508:
                raise RuntimeError("move failed")
        with patch.object(settings, "run", side_effect=fail) as execute, patch.object(settings.time, "sleep"):
            with self.assertRaisesRegex(RuntimeError, "move failed"):
                driver.drag(window, Path("."), [508, 713, 336])
        self.assertEqual(execute.call_args.args, ("xdotool", "mouseup", 1))

    def test_mac_maps_capture_pixels_to_window_points(self):
        with TemporaryDirectory() as temporary:
            directory = Path(temporary)
            driver = self.driver(directory, "macos")
            window = Mock()

            def screenshot(path):
                path.with_suffix(".window.json").write_text(json.dumps(dict(
                    kCGWindowBounds=dict(X=100, Y=50, Width=1200, Height=820), capturePixelsPerWindowPoint=2)))
            window.screenshot.side_effect = screenshot
            helper = Mock()
            with patch.object(settings, "run"), patch.object(settings, "OwnedMacInput", return_value=helper), \
                    patch.object(settings.time, "sleep"):
                driver.drag(window, directory, [1016, 1426, 672])
            helper.drag.assert_called_once_with(608.0, 763.0, 436.0)

    def test_the_mac_helper_drags_with_the_left_button_and_always_releases(self):
        helper = object.__new__(mac_input.OwnedMacInput)
        helper.release_modifiers = Mock()
        helper.post_mouse = Mock()
        helper.drag(600, 700, 440, steps=4)
        kinds = [c.args[0] for c in helper.post_mouse.call_args_list]
        self.assertEqual(kinds, [5, 1, 6, 6, 6, 6, 2])
        self.assertEqual(helper.post_mouse.call_args_list[-1], call(2, 440, 700))

        helper.post_mouse = Mock(side_effect=[None, None, RuntimeError("post failed"), None])
        with self.assertRaisesRegex(RuntimeError, "post failed"):
            helper.drag(600, 700, 440, steps=4)
        self.assertEqual(helper.post_mouse.call_args_list[-1], call(2, 440, 700))

    def test_the_windows_helper_handles_the_drag_command(self):
        script = (HERE / "windows-actions.ps1").read_text(encoding="utf-8")
        self.assertIn("'drag' {", script)
        # The button is held on every move and released even when a move fails.
        self.assertIn("PostMessageW($h, 0x200, 1,", script)
        self.assertIn("finally {\n                    [Drive]::PostMessageW($h, 0x202, 0,", script)


class GitHubScene(unittest.TestCase):
    def test_the_installed_github_scene_runs_the_step_after_the_status_bar(self):
        class Reached(Exception):
            pass
        with TemporaryDirectory() as temporary:
            output = Path(temporary) / "profile"
            names = []

            def drive(name, *args, **kwargs):
                names.append(name)
                config = output / "config.json"
                value = json.loads(config.read_text())
                value["native"]["github"] = dict(viewModes={"pr-list-my-prs": "list"})
                if name == "navigation-dark":
                    value["ui"]["showBookmarkedOnly"] = True
                    value["ui"]["favoriteUsers"] = ["fixture-author"]
                config.write_text(json.dumps(value))
                directory = output / name
                directory.mkdir()
                return directory

            driver = Mock()
            driver.drive.side_effect = drive
            backend = Mock()
            backend.query.side_effect = lambda name: dict(viewModes={"pr-list-my-prs": "list"}) \
                if name == "settings:get" else ["bookmark"] if name == "repoBookmarks:list" else {}
            api = Mock(requests=[dict(method="POST", path="/issues/7/comments", account="fixture-one",
                body=dict(body="Installed native fixture comment"))])
            with patch.object(github, "installed_binary", return_value={}), patch.object(github, "mutate"), \
                    patch.object(github, "verify_detail_requests"), patch.object(github, "require_activity", return_value={}), \
                    patch.object(github, "check") as check, patch.object(github, "check_status_bar"), \
                    patch.object(github, "qualify_review_sound"), \
                    patch.object(github.settings, "Driver", return_value=driver), \
                    patch.object(github.sidebar_layout, "qualify", side_effect=Reached) as step:
                with self.assertRaises(Reached):
                    github.qualify(Path("owned-buddy"), output, "x11", "fixture", backend, api)
            self.assertEqual(names[-1], "status-bar-dark")
            args = step.call_args.args
            self.assertEqual(args, (driver, output / "config.json", check, github.settings.run))
            # The status-bar step's assistant is closed again before the split is measured.
            self.assertIs(json.loads((output / "config.json").read_text())["ui"]["assistantOpen"], False)


if __name__ == "__main__":
    unittest.main()
