"""The tab menu scene's contract: its passes, captures, sizes and backend claim.

Nothing here launches the app. The action lists are replayed against a small
model of the tab rules in rust/crates/buddy-app/src/shell (navigation.rs and
tab_bar.rs), so the scene cannot expect a command to act where the app
disables it, or the reverse.
"""
from contextlib import nullcontext
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("tab_menu_desktop", HERE / "tab-menu-desktop.py")
scene = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scene)

LEFT_CLICKS = ("click", "content", "workspace", "sidebar", "nav", "tab", "between", "input", "field", "box")
# Kinds both the Windows command encoding and the Unix driver implement.
SHARED_KINDS = {"content", "wait_content", "wait_text", "workspace", "workspace_menu", "resize", "sleep",
    "capture", "key"}


class Tabs:
    """navigation.rs `Tabs` and tab_bar.rs `tab_menu`, for the titles the scene uses."""

    def __init__(self):
        # Every launch restores the Dashboard and the saved GitHub section.
        self.items, self.active, self.menu = [scene.DASHBOARD, scene.MY_PRS], scene.MY_PRS, None

    def disabled(self, command):
        index = self.items.index(self.menu)
        return {scene.CLOSE_OTHERS: len(self.items) <= 1,
            scene.CLOSE_RIGHT: index + 1 == len(self.items)}.get(command, False)

    def close(self, title):
        index = self.items.index(title)
        self.items.remove(title)
        if self.active == title:
            self.active = self.items[min(index, len(self.items) - 1)] if self.items else None

    def choose(self, command):
        """Returns whether the command ran, which also closes its menu."""
        if self.disabled(command):
            return False
        index = self.items.index(self.menu)
        if command == scene.CLOSE_RIGHT:
            closing = self.items[index + 1:]
        elif command == scene.CLOSE_OTHERS:
            closing = [title for title in self.items if title != self.menu]
        elif command == "Move Left":
            closing = []
            target = max(index - 1, 0)
            self.items[index], self.items[target] = self.items[target], self.items[index]
        else:
            raise AssertionError(f"The scene chose an unmodelled command: {command}")
        for title in closing:
            self.close(title)
        self.menu = None
        return True


def replay(actions):
    """Run a pass through the model; return each capture's tabs, active tab and open menu."""
    tabs, frames, journal = Tabs(), {}, []
    for index, (kind, *values) in enumerate(actions):
        following = actions[index + 1] if index + 1 < len(actions) else None
        if kind == "content" and values[0] == scene.REPOSITORIES:
            assert tabs.menu is None
            tabs.items.append(scene.REPOSITORIES)
            tabs.active = scene.REPOSITORIES
        elif kind == "workspace":
            assert tabs.menu is None and values[0] in tabs.items
            tabs.active = values[0]
        elif kind == "workspace_menu":
            assert tabs.menu is None and values[0] in tabs.items
            tabs.menu = values[0]
            journal.append(("menu", values[0], tabs.active, list(tabs.items)))
        elif kind == "content":
            assert tabs.menu is not None, "A command needs an open menu"
            command, target = values[0], tabs.menu
            ran = tabs.choose(command)
            journal.append(("command", command, target, ran, following))
        elif kind == "key":
            assert values == ["Escape"] and tabs.menu is not None
            tabs.menu = None
            journal.append(("escape",))
        elif kind == "wait_text":
            assert values == [scene.MENU_OPEN]
            assert tabs.menu is not None, "The scene waits for a menu the app would have closed"
        elif kind == "capture":
            assert values[0] not in frames, "Capture names must be unique within a pass"
            frames[values[0]] = dict(tabs=list(tabs.items), active=tabs.active, menu=tabs.menu)
    assert tabs.menu is None, "A pass must not quit with a menu open"
    return frames, journal


class Recorder:
    """Stands in for github.check and require_tab_row, keeping what each frame must show."""

    def __init__(self):
        self.text, self.rows = {}, {}

    def check(self, path, *markers, absent=()):
        self.text.setdefault(path.stem, []).append((markers, tuple(absent)))

    def row(self, path, present, absent=()):
        assert path.stem not in self.rows
        self.rows[path.stem] = (tuple(present), tuple(absent))


class ActionOrder(unittest.TestCase):
    def passes(self):
        return dict(dark=scene.dismissed_actions("dark", False), light=scene.dismissed_actions("light", True),
            commands=scene.command_actions())

    def test_every_pass_opens_three_tabs_before_its_first_menu_without_recording_activity(self):
        self.assertEqual(scene.TABS, ("Dashboard", "My PRs", "Repositories"))
        for name, actions in self.passes().items():
            with self.subTest(name=name):
                self.assertEqual(actions[:2], [("content", "Repositories"), ("wait_content", "fixture-org/native-demo")])
                _, journal = replay(actions)
                first = next(entry for entry in journal if entry[0] == "menu")
                self.assertEqual(first[3], list(scene.TABS))
                # A sidebar PR mode is the app's one activity write here.
                self.assertNotIn("sidebar", [action[0] for action in actions])
                self.assertLessEqual({action[0] for action in actions}, SHARED_KINDS)

    def test_each_first_menu_opens_on_an_inactive_tab(self):
        for name, actions in self.passes().items():
            with self.subTest(name=name):
                _, journal = replay(actions)
                _, target, active, _ = next(entry for entry in journal if entry[0] == "menu")
                self.assertNotEqual(target, active)
        dark = next(action for action in self.passes()["dark"] if action[0] == "workspace_menu")
        light = next(action for action in self.passes()["light"] if action[0] == "workspace_menu")
        # The last tab shows Close to the Right disabled; the middle tab has every command.
        self.assertEqual((dark[1], light[1]), ("Repositories", "My PRs"))

    def test_the_dismissal_passes_only_open_and_escape_a_menu_over_the_dashboard(self):
        for theme, compact in (("dark", False), ("light", True)):
            with self.subTest(theme=theme):
                actions = scene.dismissed_actions(theme, compact)
                opened = next(index for index, action in enumerate(actions) if action[0] == "workspace_menu")
                self.assertEqual(actions[opened - 1], ("capture", f"tabs-{theme}"))
                self.assertEqual(actions[opened + 1:], [("wait_text", scene.MENU_OPEN), ("capture", f"menu-{theme}"),
                    ("key", "Escape"), ("sleep", 1), ("capture", f"dismissed-{theme}")])
                # Nothing but the right-click and Escape touches the app once the menu is open.
                self.assertFalse([action for action in actions[opened + 1:] if action[0] in LEFT_CLICKS])
                frames, journal = replay(actions)
                self.assertEqual([entry[0] for entry in journal], ["menu", "escape"])
                for name in (f"tabs-{theme}", f"menu-{theme}", f"dismissed-{theme}"):
                    self.assertEqual((frames[name]["tabs"], frames[name]["active"]),
                        (list(scene.TABS), scene.DASHBOARD))
                self.assertEqual([frames[name]["menu"] for name in frames],
                    [None, actions[opened][1], None])

    def test_dark_stays_at_the_large_size_and_light_is_captured_compact(self):
        dark, light = scene.dismissed_actions("dark", False), scene.dismissed_actions("light", True)
        self.assertNotIn("resize", [action[0] for action in dark + scene.command_actions()])
        resized = light.index(("resize", 900, 650))
        self.assertLess(resized, light.index(("capture", "tabs-light")))
        self.assertEqual([action for action in light if action[0] == "resize"], [("resize", 900, 650)])
        # The tabs are opened and the Dashboard chosen at the size every other scene uses.
        self.assertLess(light.index(("workspace", "Dashboard")), resized)
        # The real Windows encoding starts each pass at the large size and knows every action.
        with tempfile.TemporaryDirectory(prefix="buddy-tab-menu-sizes-") as root:
            directory = Path(root)
            driver = object.__new__(scene.settings.Driver)
            driver.profile, driver.session, driver.large_size = directory, "windows", (1200, 820)
            driver.executable, driver.config, driver.checkout = directory / "owned.exe", directory / "config.json", directory
            driver.backend = Mock(url="http://127.0.0.1:9")
            sizes = {}
            for name, actions in (("dark", dark), ("light", light), ("commands", scene.command_actions())):
                with patch.object(scene.settings, "run") as run, patch.object(scene.settings, "encode_recording"):
                    driver.drive(name, actions, record=True)
                commands = run.call_args.args[run.call_args.args.index("-Actions") + 1].split(",")
                menu = next(index for index, command in enumerate(commands) if command.startswith("workspace_menu64:"))
                sizes[name] = [command for command in commands[:menu] if command.startswith("resize:")]
                self.assertEqual((commands[1].split(":")[0], commands[-2]), ("record-start", "record-stop"))
                # The fixed 120-second Windows action window covers every explicit pause.
                self.assertLessEqual(sum(action[1] for action in actions if action[0] == "sleep"), 10)
        self.assertEqual(sizes["dark"], ["resize:80:40:1200:820"])
        self.assertEqual(sizes["commands"], ["resize:80:40:1200:820"])
        self.assertEqual(sizes["light"], ["resize:80:40:1200:820", "resize:80:40:900:650"])

    def test_bulk_commands_act_where_enabled_and_leave_the_menu_open_where_disabled(self):
        actions = scene.command_actions()
        frames, journal = replay(actions)
        commands = [entry for entry in journal if entry[0] == "command"]
        self.assertEqual([(command, target, ran) for _, command, target, ran, _ in commands], [
            (scene.CLOSE_RIGHT, scene.MY_PRS, True), (scene.CLOSE_RIGHT, scene.MY_PRS, False),
            (scene.CLOSE_OTHERS, scene.MY_PRS, True), (scene.CLOSE_OTHERS, scene.MY_PRS, False),
            ("Move Left", scene.MY_PRS, True)])
        for _, command, _, ran, following in commands:
            # A command that ran closed its menu: the scene pauses, then captures.
            # A disabled one did not: the scene must still read the open menu.
            self.assertEqual(following, ("sleep", 2 if command == scene.CLOSE_RIGHT else 1) if ran
                else ("wait_text", scene.MENU_OPEN), command)
        self.assertEqual({name: frame["tabs"] for name, frame in frames.items()}, {
            "three-tabs-dark": ["Dashboard", "My PRs", "Repositories"],
            "right-enabled-dark": ["Dashboard", "My PRs", "Repositories"],
            "closed-right-dark": ["Dashboard", "My PRs"], "right-disabled-dark": ["Dashboard", "My PRs"],
            "closed-others-dark": ["My PRs"], "others-disabled-dark": ["My PRs"], "single-tab-dark": ["My PRs"]})
        self.assertEqual([name for name, frame in frames.items() if frame["menu"]],
            ["right-enabled-dark", "right-disabled-dark", "others-disabled-dark"])
        # Every menu is confirmed open before its capture or its command.
        for index, action in enumerate(actions):
            if action[0] == "workspace_menu":
                self.assertEqual(actions[index + 1], ("wait_text", scene.MENU_OPEN))

    def test_the_menu_wait_names_only_commands_no_tab_count_disables(self):
        self.assertEqual(scene.MENU_OPEN.split("|"), list(scene.ALWAYS_ENABLED))
        tabs = Tabs()
        tabs.items, tabs.menu = [scene.MY_PRS], scene.MY_PRS
        self.assertEqual([command for command in scene.COMMANDS if not tabs.disabled(command)],
            list(scene.ALWAYS_ENABLED))
        # tab_bar.rs is the source of the six labels and of both rules.
        source = (HERE.parent.parent / "rust/crates/buddy-app/src/shell/tab_bar.rs").read_text(encoding="utf-8")
        for command in scene.COMMANDS:
            self.assertIn(f'("{command}", ', source)
        self.assertIn('("Close Others", 1, count <= 1)', source)
        self.assertIn('("Close to the Right", 2, index + 1 == count)', source)


class RequiredCaptures(unittest.TestCase):
    def read(self, verify, actions):
        recorder = Recorder()
        with patch.object(scene.github, "check", recorder.check), \
                patch.object(scene, "require_tab_row", recorder.row):
            verify(Path("flow"))
        frames, _ = replay(actions)
        return recorder, frames

    def test_every_capture_is_checked_and_every_check_reads_a_real_capture(self):
        for verify, actions in ((lambda flow: scene.check_dismissed(flow, "dark", False), scene.dismissed_actions("dark", False)),
                (lambda flow: scene.check_dismissed(flow, "light", True), scene.dismissed_actions("light", True)),
                (scene.check_commands, scene.command_actions())):
            recorder, frames = self.read(verify, actions)
            self.assertEqual(set(recorder.text), set(frames))
            self.assertLessEqual(set(recorder.rows), set(frames))

    def test_menu_frames_require_the_menu_and_every_other_frame_forbids_it(self):
        for verify, actions in ((lambda flow: scene.check_dismissed(flow, "dark", False), scene.dismissed_actions("dark", False)),
                (lambda flow: scene.check_dismissed(flow, "light", True), scene.dismissed_actions("light", True)),
                (scene.check_commands, scene.command_actions())):
            recorder, frames = self.read(verify, actions)
            for name, frame in frames.items():
                markers = {marker for required, _ in recorder.text[name] for marker in required}
                absent = {marker for _, forbidden in recorder.text[name] for marker in forbidden}
                with self.subTest(name=name):
                    if frame["menu"]:
                        self.assertLessEqual(set(scene.ALWAYS_ENABLED), markers)
                        self.assertFalse(absent & set(scene.COMMANDS))
                    else:
                        self.assertLessEqual(set(scene.ALWAYS_ENABLED), absent)
                        self.assertFalse(markers & set(scene.COMMANDS))

    def test_a_dimmed_command_is_never_a_required_marker(self):
        for verify, actions in ((lambda flow: scene.check_dismissed(flow, "dark", False), scene.dismissed_actions("dark", False)),
                (lambda flow: scene.check_dismissed(flow, "light", True), scene.dismissed_actions("light", True)),
                (scene.check_commands, scene.command_actions())):
            recorder, frames = self.read(verify, actions)
            for name, frame in frames.items():
                if not frame["menu"]:
                    continue
                tabs = Tabs()
                tabs.items, tabs.menu = frame["tabs"], frame["menu"]
                markers = {marker for required, _ in recorder.text[name] for marker in required}
                for command in scene.COMMANDS:
                    if tabs.disabled(command):
                        self.assertNotIn(command, markers, name)

    def test_both_themes_show_the_full_enabled_menu_over_the_dashboard(self):
        for theme, compact in (("dark", False), ("light", True)):
            recorder, frames = self.read(lambda flow: scene.check_dismissed(flow, theme, compact),
                scene.dismissed_actions(theme, compact))
            (markers, absent), = recorder.text[f"menu-{theme}"]
            tabs = Tabs()
            tabs.items, tabs.menu = frames[f"menu-{theme}"]["tabs"], frames[f"menu-{theme}"]["menu"]
            enabled = [command for command in scene.COMMANDS if not tabs.disabled(command)]
            self.assertEqual(list(markers), [*enabled, scene.DASHBOARD_ONLY])
            # The active tab did not change: the Dashboard, not the GitHub view, is behind the menu.
            self.assertEqual(absent, (scene.GITHUB_ONLY,))
            for name in (f"tabs-{theme}", f"dismissed-{theme}"):
                self.assertTrue(all(scene.DASHBOARD_ONLY in required for required, _ in recorder.text[name]))
            self.assertIn(scene.GITHUB_ONLY, recorder.text[f"dismissed-{theme}"][0][1])
            # Escape left all three tabs open.
            self.assertEqual(recorder.rows, {f"dismissed-{theme}": (scene.TABS, ())})

    def test_tab_row_checks_follow_the_tabs_each_command_leaves(self):
        recorder, frames = self.read(scene.check_commands, scene.command_actions())
        self.assertEqual(list(recorder.rows),
            ["three-tabs-dark", "closed-right-dark", "closed-others-dark", "single-tab-dark"])
        for name, (present, absent) in recorder.rows.items():
            self.assertEqual(list(present), frames[name]["tabs"], name)
            self.assertEqual(set(absent), set(scene.TABS) - set(present), name)
        # A closed title was read as open by the same reader earlier in the pass.
        self.assertEqual(recorder.rows["three-tabs-dark"], (scene.TABS, ()))


def word(text, left, top, line, width=40, height=10):
    return dict(text=text, left=left, top=top, width=width, height=height, line=line)


class TabRow(unittest.TestCase):
    def fixture(self, name):
        return json.loads((HERE / "fixtures" / "github-review-tooltip-occlusion.json").read_text(encoding="utf-8"))[name]

    def require(self, words, present, absent=(), **patches):
        # These frames have no image file. A band re-read holds only the tab
        # row, so it sees the row's words and none of the view below it.
        with tempfile.TemporaryDirectory(prefix="buddy-tab-row-") as root, \
                patch.object(scene.settings.ocr, "recognize", return_value=words), \
                patch.object(scene.settings.ocr, "recognize_windows", return_value=words), \
                patch.object(scene, "tab_band_words", side_effect=lambda _capture: scene.settings.ocr.workspace_words(words)):
            capture = Path(root) / "frame.png"
            with patch.multiple(scene.settings.ocr, **patches) if patches else nullcontext():
                scene.require_tab_row(capture, present, absent)
            return json.loads(capture.with_suffix(".tab-row-ocr.json").read_text(encoding="utf-8"))

    def test_recorded_frames_distinguish_open_tabs_from_the_sidebar_below_them(self):
        # Hosted Ubuntu frames: three tabs open, then two; the GitHub sidebar
        # lists Needs Review in both.
        clear, two = self.fixture("clear"), self.fixture("occluded")
        self.assertEqual(self.require(clear, ("Dashboard", "My PRs", "Needs Review")), clear)
        self.require(two, ("Dashboard", "My PRs"), absent=("Needs Review",))
        self.assertTrue(any(item["text"] == "Review" for item in two))
        with self.assertRaisesRegex(RuntimeError, "'Needs Review' workspace tab is still open"):
            self.require(clear, ("Dashboard", "My PRs"), absent=("Needs Review",))
        with self.assertRaisesRegex(RuntimeError, "None of the requested visible labels"):
            self.require(two, ("Dashboard", "My PRs", "Needs Review"))

    def test_a_closed_tab_needs_an_open_one_read_from_the_same_frame(self):
        with self.assertRaisesRegex(RuntimeError, "another title"):
            scene.require_tab_row(Path("unread.png"), (), absent=("Dashboard",))
        # An unreadable row fails on its open titles; it never passes as closed tabs.
        with self.assertRaises(RuntimeError):
            self.require([], ("My PRs",), absent=("Dashboard",),
                workspace_geometry=Mock(side_effect=RuntimeError("The workspace has no measured chrome bands")))

    def test_an_unread_menu_row_falls_back_to_the_painted_tab_band(self):
        # Sparse OCR can read File as Fie on Mac; the tab locator then measures the band.
        words = [word("Fie", 96, 11, "1"), word("Dashboard", 79, 44, "2", 82), word("My", 246, 44, "2", 15),
            word("PRs", 265, 44, "2", 23), word("My", 109, 87, "3", 19), word("PRs", 134, 87, "3", 24),
            word("Repositories", 286, 134, "4", 85)]
        geometry = Mock(return_value=(33, 62))
        self.require(words, ("Dashboard", "My PRs"), absent=("Repositories",), workspace_geometry=geometry)
        geometry.assert_called_once()
        with self.assertRaisesRegex(RuntimeError, "still open"):
            self.require(words + [word("Repositories", 380, 44, "2", 85)], ("My PRs",), absent=("Repositories",),
                workspace_geometry=geometry)

    def focus_ring(self):
        return json.loads((HERE / "fixtures" / "tab-menu-focus-ring-row.json").read_text(encoding="utf-8"))

    def require_two_reads(self, full, band, present, absent=()):
        """The full-frame read, then the cropped band; return how often the band was read."""
        with tempfile.TemporaryDirectory(prefix="buddy-tab-band-") as root, \
                patch.object(scene.settings.ocr, "recognize", side_effect=[full, band]), \
                patch.object(scene.settings.ocr, "recognize_windows", side_effect=[full, band]), \
                patch.object(scene.settings.ocr, "workspace_geometry", return_value=(34, 68)) as geometry, \
                patch.object(scene, "crop_band", side_effect=lambda capture, top, bottom: capture.with_suffix(".tab-band.png")):
            scene.require_tab_row(Path(root) / "dismissed-light.png", present, absent)
            return geometry.call_count

    def test_a_focus_ring_garbled_title_is_read_again_from_the_tab_band(self):
        fixture = self.focus_ring()
        # The hosted full-frame read alone cannot find the focused tab.
        with self.assertRaises(RuntimeError):
            scene.settings.ocr.locate_label(scene.settings.ocr.workspace_words(fixture["full"]), "My PRs")
        self.assertEqual(self.require_two_reads(fixture["full"], fixture["band"], scene.TABS), 1)
        # Both reads missing a title still fails.
        with self.assertRaises(RuntimeError):
            self.require_two_reads(fixture["full"], [w for w in fixture["band"] if w["text"] != "PRs)"], scene.TABS)

    def test_closed_titles_are_judged_on_the_read_that_found_the_open_ones(self):
        fixture = self.focus_ring()
        # Repositories is open in the band read, so it cannot count as closed.
        with self.assertRaisesRegex(RuntimeError, "'Repositories' workspace tab is still open"):
            self.require_two_reads(fixture["full"], fixture["band"], ("Dashboard", "My PRs"), absent=("Repositories",))
        band = [w for w in fixture["band"] if w["text"] not in ("GitHub:", "Repositories")]
        self.assertEqual(self.require_two_reads(fixture["full"], band, ("Dashboard", "My PRs"), absent=("Repositories",)), 1)

    def test_two_tabs_with_one_title_are_never_resolved_by_order(self):
        words = [word("File", 96, 11, "1", 19), word("View", 160, 11, "1", 29), word("Help", 200, 11, "1", 26),
            word("Dashboard", 79, 44, "2", 82), word("x", 165, 44, "2", 8), word("Dashboard", 180, 44, "2", 82),
            word("x", 266, 44, "2", 8), word("My", 282, 44, "2", 15), word("PRs", 301, 44, "2", 23)]
        for present, absent in ((("My PRs",), ("Dashboard",)), (("My PRs", "Dashboard"), ())):
            with self.assertRaises(scene.settings.ocr.AmbiguousLabel):
                self.require(words, present, absent)


class Backend:
    def __init__(self, snapshots):
        self.snapshots, self.paths = list(snapshots), []

    def query(self, path):
        self.paths.append(path)
        return self.snapshots.pop(0)


def stats(**changes):
    return {**{key: 3 for key in scene.COUNTERS}, "lastSessionStart": 1792152000000, **changes}


def passes():
    """The baseline, then each pass's snapshot: one launch, with its Dashboard
    open and one closed session, and one repository-list open per pass."""
    snapshots = [stats(reposBrowsed=3 + count, tabsOpened=3 + count, appLaunches=3 + count,
        totalUptimeMs=3 + 1000 * count) for count in range(4)]
    for snapshot in snapshots:
        del snapshot["lastSessionStart"]
    return snapshots


READS = [dict(method="POST", path="/graphql", account="fixture-one", query="query NativeViewerPulls", body=None),
    dict(method="GET", path="/orgs/fixture-org/repos?per_page=100", account="fixture-one", query="", body=None)]


class NoBackendChange(unittest.TestCase):
    def qualify(self, snapshots, requests=READS, session="x11"):
        drives = []

        class Driver:
            large_size = (1200, 820)

            def __init__(self, executable, profile, driver_session, backend, checkout):
                self.profile = profile

            def drive(self, name, actions, concurrent=False, record=False):
                config = json.loads((self.profile / "config.json").read_text(encoding="utf-8"))
                drives.append(dict(name=name, actions=actions, record=record, theme=config["ui"]["theme"],
                    section=config["native"]["activeSection"], accounts=config["github"]["accounts"],
                    panes=config["ui"].get("paneSizes"),
                    marker=(self.profile / "github-fixture.allow").read_text(),
                    queries=len(backend.paths)))
                # The app saves the section it quits on. The Dashboard is no
                # section, and `NativeConfig::is_default` then drops the whole
                # `native` object from the file, as it did on Ubuntu in run
                # 44854538756.
                frames, _ = replay(actions)
                last = list(frames.values())[-1]["active"]
                if last == scene.DASHBOARD:
                    del config["native"]
                else:
                    config["native"] = dict(activeSection="github")
                (self.profile / "config.json").write_text(json.dumps(config), encoding="utf-8")
                return self.profile / name

        backend = Backend(snapshots)
        with tempfile.TemporaryDirectory(prefix="buddy-tab-menu-scene-") as root, \
                patch.object(scene.settings, "Driver", Driver), patch.object(scene.github, "check"), \
                patch.object(scene, "require_tab_row"), patch.object(scene.github, "mutate") as mutate, \
                patch.object(scene.github, "installed_binary", return_value=dict(versionOutput="Buddy Native 1.2.3",
                    binarySha256="a" * 64)):
            output = Path(root) / "tab-menu"
            try:
                scene.qualify(Path(root) / "buddy-native", output, session, "1.2.3", backend, Mock(requests=list(requests)))
            finally:
                self.drives, self.mutations, self.backend = drives, mutate.call_args_list, backend
            return {path.name: json.loads(path.read_text(encoding="utf-8")) for path in output.glob("*.json")}

    def test_three_recorded_passes_run_dark_dark_light_against_one_backend_baseline(self):
        receipts = self.qualify(passes())
        self.assertEqual([(drive["name"], drive["theme"], drive["record"]) for drive in self.drives],
            [("menu-dark", "dark", True), ("commands-dark", "dark", True), ("menu-light", "light", True)])
        self.assertEqual([drive["actions"] for drive in self.drives], [scene.dismissed_actions("dark", False),
            scene.command_actions(), scene.dismissed_actions("light", True)])
        # The baseline is read before the first launch, and once more after each pass.
        self.assertEqual([drive["queries"] for drive in self.drives], [1, 2, 3])
        self.assertEqual(self.backend.paths, ["buddyStats:get"] * 4)
        # The first pass quits on the Dashboard; the next still launches into GitHub.
        for drive in self.drives:
            # Every pass, light included, keeps the sidebar out from under the menus.
            self.assertEqual(drive["panes"], list(scene.PANE_SIZES))
            self.assertEqual((drive["section"], drive["marker"]), ("github", "hs-buddy native GitHub qualification v1"))
            self.assertEqual([account["username"] for account in drive["accounts"]], list(scene.ACCOUNTS))
        self.assertEqual([mutation.args[1:] for mutation in self.mutations],
            [("githubAccounts:create", dict(username=user, org="fixture-org")) for user in scene.ACCOUNTS])
        receipt = receipts["qualification.json"]
        self.assertEqual((receipt["result"], receipt["session"], receipt["version"]), ("passed", "x11", "1.2.3"))
        self.assertEqual(receipt["binarySha256"], "a" * 64)
        self.assertLessEqual({"dark 1200x820", "light 900x650", "interaction recordings",
            "only repository-list activity in any pass", "no GitHub write"}, set(receipt["checks"]))
        activity = receipts["backend-activity.json"]
        self.assertEqual(list(activity["passes"]), ["menu-dark", "commands-dark", "menu-light"])
        for count, value in enumerate(activity["passes"].values(), 1):
            self.assertEqual(value["browsed"], 1)
            self.assertEqual(value["launches"], count)
            self.assertEqual(value["changes"], {key: count if key in ("reposBrowsed", "tabsOpened", "appLaunches")
                else 1000 * count if key == "totalUptimeMs" else 0 for key in scene.COUNTERS})
        self.assertEqual(receipts["requests.json"], READS)

    def test_any_activity_counter_change_in_any_pass_fails_the_scene(self):
        for failing, name in enumerate(("menu-dark", "commands-dark", "menu-light")):
            changes = dict(extra_tab=lambda snapshot: dict(tabsOpened=snapshot["tabsOpened"] + 1),
                missing_dashboard=lambda snapshot: dict(tabsOpened=snapshot["tabsOpened"] - 1),
                foreign=lambda _: dict(prsViewed=4),
                missing_launch=lambda snapshot: dict(appLaunches=snapshot["appLaunches"] - 1),
                no_uptime=lambda _: dict(totalUptimeMs=3), open_session=lambda _: dict(lastSessionStart=1),
                missing_list=lambda snapshot: dict(reposBrowsed=snapshot["reposBrowsed"] - 1),
                extra_list=lambda snapshot: dict(reposBrowsed=snapshot["reposBrowsed"] + 1))
            for change_name, change in changes.items():
                with self.subTest(name=name, change=change_name):
                    good = passes()
                    snapshots = good[:failing + 1] + [dict(snapshot, **change(snapshot)) for snapshot in good[failing + 1:]]
                    with self.assertRaisesRegex(RuntimeError, f"changed the backend in {name}"):
                        self.qualify(snapshots)
                    # The scene stops at the pass that wrote; it leaves no passing receipt.
                    self.assertEqual(len(self.drives), failing + 1)

    def test_a_write_left_by_an_earlier_pass_is_still_found_against_the_baseline(self):
        # Each pass alone looks unchanged from the one before it.
        good = passes()
        changed = [dict(snapshot, tabsOpened=snapshot["tabsOpened"] + 1) for snapshot in good[2:]]
        with self.assertRaisesRegex(RuntimeError, "changed the backend in commands-dark"):
            self.qualify(good[:2] + changed)

    def test_unchanged_means_every_counter_and_the_session_start(self):
        self.assertEqual(scene.require_unchanged(stats(), stats(), "pass"), {key: 0 for key in scene.COUNTERS})
        # Only the counted repository-list opens may move reposBrowsed.
        self.assertEqual(scene.require_unchanged(stats(), stats(reposBrowsed=5), "pass", 2)["reposBrowsed"], 2)
        for browsed in (1, 3):
            with self.assertRaisesRegex(RuntimeError, "changed the backend in pass"):
                scene.require_unchanged(stats(), stats(reposBrowsed=5), "pass", browsed)
        self.assertIn("tabsOpened", scene.COUNTERS)
        # Two launches: two Dashboard opens and two closed sessions.
        launched = stats(tabsOpened=5, appLaunches=5, totalUptimeMs=4000)
        del launched["lastSessionStart"]
        self.assertEqual(scene.require_unchanged(stats(), launched, "pass", 0, 2)["tabsOpened"], 2)
        for wrong in (dict(launched, tabsOpened=4), dict(launched, appLaunches=4),
                dict(launched, totalUptimeMs=3), dict(launched, lastSessionStart=1)):
            with self.subTest(wrong=wrong), self.assertRaisesRegex(RuntimeError, "changed the backend in pass"):
                scene.require_unchanged(stats(), wrong, "pass", 0, 2)
        for key in scene.COUNTERS:
            with self.assertRaisesRegex(RuntimeError, "changed the backend in pass"):
                scene.require_unchanged(stats(), stats(**{key: 4}), "pass")
            # A counter the backend has not created yet counts as zero.
            before = stats()
            del before[key]
            with self.assertRaises(RuntimeError):
                scene.require_unchanged(before, stats(), "pass")

    def test_a_github_write_fails_the_scene_and_reads_do_not(self):
        scene.require_reads_only(READS)
        writes = (dict(method="POST", path="/repos/fixture-org/native-demo/issues/7/comments", query=""),
            dict(method="POST", path="/graphql", query="mutation NativeResolveThread"),
            dict(method="PATCH", path="/repos/fixture-org/native-demo/pulls/7", query=""),
            dict(method="DELETE", path="/repos/fixture-org/native-demo/issues/comments/1", query=""))
        for write in writes:
            with self.subTest(write=write["path"]), self.assertRaisesRegex(RuntimeError, "wrote to the synthetic GitHub API"):
                scene.require_reads_only(READS + [write])
        with self.assertRaisesRegex(RuntimeError, "wrote to the synthetic GitHub API"):
            self.qualify(passes(), requests=READS + [writes[0]])
        self.assertEqual(len(self.drives), 3)


class HostGuard(unittest.TestCase):
    def test_non_runner_is_refused_before_argument_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(scene.argparse, "ArgumentParser") as parser:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                scene.main()
            parser.assert_not_called()

    def test_the_scene_is_registered_for_every_hosted_package(self):
        unix = (HERE / "install-unix.sh").read_text(encoding="utf-8")
        darwin = unix.index('if [[ "$(uname -s)" == Darwin ]]; then')
        self.assertIn('python3 "$1/tab-menu-desktop.py" --executable "$(command -v buddy-native)" '
            '--output "$2/tab-menu" --session x11 --version "$3"', unix[:darwin])
        self.assertIn('scene tab-menu python3 "$script_directory/tab-menu-desktop.py" '
            '--executable "$app/Contents/MacOS/buddy-native" --output "$output/tab-menu" --session macos',
            unix[darwin:])
        windows = (HERE / "install-windows.ps1").read_text(encoding="utf-8")
        self.assertIn("'tab-menu-desktop.py') --executable $exe --output (Join-Path $Output 'tab-menu') "
            "--session windows", windows)
        self.assertIn("throw 'Installed tab menu qualification failed'", windows)


if __name__ == "__main__":
    unittest.main()
