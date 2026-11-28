import importlib.util
import json
import os
from pathlib import Path
import socket
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import urlparse

spec = importlib.util.spec_from_file_location("bookmarks_desktop", Path(__file__).with_name("bookmarks-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def record(value):
    return dict(value, _id="id-" + value["url"], sortOrder=0, createdAt=1, updatedAt=1)


def after_exercise(deleted=("Old dark link", "Old light link")):
    kept = [record(dict(url=url, title=title, category=category, tags=tags))
        for url, title, category, tags, _ in desktop.SEED if title not in deleted]
    return kept + [record(desktop.created(theme)) for theme in desktop.THEMES]


class BookmarksDesktopTests(unittest.TestCase):
    def test_non_runner_is_refused_before_argument_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()

    def test_seed_matches_the_convex_create_arguments(self):
        arguments = list(desktop.seed_arguments())
        self.assertEqual(len(arguments), len(desktop.SEED))
        for value in arguments:
            self.assertLessEqual(set(value), {"url", "title", "category", "tags", "description"})
            self.assertIn(urlparse(value["url"]).scheme, ("http", "https"))
            self.assertTrue(value["category"].strip())
            # Convex optional fields are omitted, never sent as null or empty.
            self.assertTrue(value.get("tags", ["x"]))
            self.assertNotIn(None, value.values())
        titles = [value["title"] for value in arguments]
        self.assertEqual(len(titles), len(set(titles)), "titles identify bookmarks on screen")
        # Each theme deletes its own seeded bookmark and adds the same new one.
        self.assertIn("Old dark link", titles)
        self.assertIn("Old light link", titles)
        added = [desktop.created(theme) for theme in desktop.THEMES]
        # Convex refuses a URL twice in one category, so each pass adds its own.
        self.assertNotEqual(added[0]["url"], added[1]["url"])
        self.assertEqual(added[0]["category"], added[1]["category"])
        # The light pass lists that category with the dark pass's bookmark still
        # in it (Windows run 40621445078): a shared title is two OCR matches.
        all_titles = titles + [value["title"] for value in added]
        self.assertEqual(len(all_titles), len(set(all_titles)), "added titles identify bookmarks on screen")

    def test_seeding_refuses_a_backend_that_already_has_bookmarks(self):
        class Backend:
            def query(self, path):
                return [record(desktop.created("dark"))]
        with patch.object(desktop.github, "mutate") as mutate:
            with self.assertRaisesRegex(RuntimeError, "not empty"):
                desktop.seed(Backend())
            mutate.assert_not_called()

    def test_seeding_creates_every_fixture_through_the_incumbent_function(self):
        class Backend:
            def query(self, path):
                return []
        with patch.object(desktop.github, "mutate") as mutate:
            desktop.seed(Backend())
        self.assertEqual([call.args[1] for call in mutate.call_args_list], ["bookmarks:create"] * len(desktop.SEED))

    def test_the_backend_must_show_exactly_the_exercised_changes(self):
        deleted = {"Old dark link", "Old light link"}
        desktop.verify_backend(after_exercise(), deleted)
        with self.assertRaisesRegex(RuntimeError, "Unexpected bookmarks"):
            desktop.verify_backend(after_exercise(deleted=("Old dark link",)), deleted)
        with self.assertRaisesRegex(RuntimeError, "Unexpected bookmarks"):
            desktop.verify_backend(after_exercise()[:-1], deleted)
        wrong = after_exercise()
        wrong[-1]["category"] = "Reading"
        with self.assertRaisesRegex(RuntimeError, "saved as"):
            desktop.verify_backend(wrong, deleted)

    def test_insights_opens_each_entry_through_sidebar_and_tab_targets(self):
        dark = desktop.insights_actions("dark")
        clicks = [action[:2] for action in dark if action[0] in ("sidebar", "workspace", "content", "click")]
        # Sidebar entries are sidebar-scoped and tab labels workspace-scoped;
        # no plain view click reaches the content or the footer.
        self.assertEqual(clicks, [("sidebar", "Productivity"), ("sidebar", "Activity"),
            ("workspace", "Productivity"), ("workspace", "Activity")])
        captures = [action[1] for action in dark if action[0] == "capture"]
        self.assertEqual(captures, ["insights-productivity-dark", "insights-activity-dark"])
        # Each capture follows the entry that opens its page.
        for page in ("productivity", "activity"):
            entry = dark.index(("sidebar", page.capitalize()))
            self.assertLess(entry, dark.index(("capture", f"insights-{page}-dark")))
        light = desktop.insights_actions("light")
        self.assertEqual([action[1] for action in light if action[0] == "capture"], ["insights-productivity-light"])
        self.assertNotIn(("sidebar", "Activity"), light)

    def test_insights_restores_the_bookmarks_section_even_when_the_drive_fails(self):
        with tempfile.TemporaryDirectory() as raw:
            config = Path(raw) / "config.json"
            config.write_text(json.dumps(dict(native=dict(activeSection="bookmarks"))))
            seen = []

            class Driver:
                def drive(self, name, actions, record=False):
                    seen.append((name, json.loads(config.read_text())["native"]["activeSection"], record))
                    raise RuntimeError("drive failed")

            with self.assertRaisesRegex(RuntimeError, "drive failed"):
                desktop.insights(Driver(), config, "dark")
            self.assertEqual(seen, [("insights-dark", "insights", True)])
            self.assertEqual(json.loads(config.read_text())["native"]["activeSection"], "bookmarks")

    def test_insights_checks_the_coming_soon_text_in_each_capture(self):
        with tempfile.TemporaryDirectory() as raw:
            config = Path(raw) / "config.json"
            config.write_text(json.dumps(dict(native=dict(activeSection="bookmarks"))))

            class Driver:
                def drive(self, name, actions, record=False):
                    return Path(raw) / name

            with patch.object(desktop.github, "check") as check:
                desktop.insights(Driver(), config, "dark")
            self.assertEqual([call.args[0].name for call in check.call_args_list],
                ["insights-productivity-dark.png", "insights-activity-dark.png"])
            for call in check.call_args_list:
                self.assertIn(desktop.COMING_SOON, call.args)

    def test_the_failure_url_is_a_closed_loopback_port(self):
        url = urlparse(desktop.unreachable_url())
        self.assertEqual((url.scheme, url.hostname), ("http", "127.0.0.1"))
        with socket.socket() as probe:
            probe.settimeout(2)
            self.assertNotEqual(probe.connect_ex(("127.0.0.1", url.port)), 0)


if __name__ == "__main__":
    unittest.main()
