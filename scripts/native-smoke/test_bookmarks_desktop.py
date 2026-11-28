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


# A 1200 x 820 Insights window as Tesseract reads it: the menu row, the tab
# row, the sidebar's two entries, the content heading level with the first
# entry, the coming-soon text and the footer. Words of one row in one column
# share a line; the sidebar and the content column are separate blocks.
SIDEBAR_RIGHT = 250
TAB_ROW_BOTTOM = 57


def insights_words(shown):
    rows = [
        (10, [("File", 60), ("Edit", 95), ("View", 130), ("Help", 170)]),
        (45, [("Dashboard", 60), ("Productivity", 170), ("Activity", 290)]),
        (84, [("Productivity", 70), (shown, 290)]),
        (114, [("Activity", 70)]),
        (152, [("This", 290), ("feature", 330), ("is", 395), ("coming", 415), ("soon!", 480)]),
        (796, [("0", 60), ("PRs", 72), ("0", 110), ("schedules", 122), ("0", 200), ("jobs", 212), ("Buddy", 560)]),
    ]
    words = []
    for block, (top, row) in enumerate(rows):
        for text, left in row:
            column = "sidebar" if top > TAB_ROW_BOTTOM and left < SIDEBAR_RIGHT else "content"
            words.append(dict(text=text, left=left, top=top, width=9 * len(text), height=12,
                line=f"{block}/{column}/1"))
    return words


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

    def test_insights_opens_each_page_with_scoped_clicks_in_both_themes(self):
        for theme in desktop.THEMES:
            actions = desktop.insights_actions(theme)
            clicks = [action[:2] for action in actions if action[0] in ("sidebar", "workspace", "content", "click", "nav")]
            # Sidebar entries are content-scoped and tab labels workspace-scoped;
            # no plain view click and no GitHub-only sidebar locator.
            expected = [("content", "Activity"), ("content", "Productivity")]
            if theme == "dark":
                expected += [("workspace", "Activity"), ("workspace", "Productivity")]
            self.assertEqual(clicks, expected)
            captures = [action[1] for action in actions if action[0] == "capture"]
            self.assertEqual(captures, [f"insights-activity-{theme}", f"insights-productivity-{theme}"])
            # Each capture follows the click that opens its page.
            for page in desktop.PAGES:
                self.assertLess(actions.index(("content", page)),
                    actions.index(("capture", f"insights-{page.casefold()}-{theme}")))

    def test_each_click_names_the_one_visible_page_entry_in_a_real_layout(self):
        # Before each click the shown page's name is also its heading, so it
        # is two content matches; the other page's name is the sidebar entry alone.
        for shown, other in (("Productivity", "Activity"), ("Activity", "Productivity")):
            words = insights_words(shown)
            with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
                desktop.settings.ocr.locate_label(words, shown, content=True)
            target = desktop.settings.ocr.locate_label(words, other, content=True)
            self.assertLess(target["left"], SIDEBAR_RIGHT)
            self.assertGreater(target["top"], TAB_ROW_BOTTOM)

    def test_the_heading_check_reads_the_content_column_only(self):
        for shown, other in (("Productivity", "Activity"), ("Activity", "Productivity")):
            words = insights_words(shown)
            self.assertEqual(desktop.content_heading(words), shown)
            self.assertTrue(desktop.heading_is(words, shown))
            # Both names are in the sidebar and the tab row; neither counts.
            self.assertFalse(desktop.heading_is(words, other))
        with self.assertRaises(RuntimeError):
            desktop.content_heading([word for word in insights_words("Activity") if word["text"] != "soon!"])

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

    def test_insights_checks_text_and_heading_in_each_capture(self):
        for theme in desktop.THEMES:
            with tempfile.TemporaryDirectory() as raw:
                config = Path(raw) / "config.json"
                config.write_text(json.dumps(dict(native=dict(activeSection="bookmarks"))))

                class Driver:
                    def drive(self, name, actions, record=False):
                        return Path(raw) / name

                with patch.object(desktop.github, "check") as check, \
                        patch.object(desktop, "check_heading") as heading:
                    desktop.insights(Driver(), config, theme)
                names = [f"insights-productivity-{theme}.png", f"insights-activity-{theme}.png"]
                self.assertEqual([call.args[0].name for call in check.call_args_list], names)
                for call in check.call_args_list:
                    self.assertIn(desktop.COMING_SOON, call.args)
                self.assertEqual([(call.args[0].name, call.args[1]) for call in heading.call_args_list],
                    list(zip(names, desktop.PAGES)))

    def test_a_capture_with_the_wrong_heading_fails(self):
        with tempfile.TemporaryDirectory() as raw:
            capture = Path(raw) / "insights-activity-dark.png"
            with patch.object(desktop.github, "analysis_images", return_value=[capture]), \
                    patch.object(desktop.settings.ocr, "recognize", return_value=insights_words("Productivity")):
                with self.assertRaisesRegex(RuntimeError, "does not show 'Activity'"):
                    desktop.check_heading(capture, "Activity")
                desktop.check_heading(capture, "Productivity")

    def test_the_failure_url_is_a_closed_loopback_port(self):
        url = urlparse(desktop.unreachable_url())
        self.assertEqual((url.scheme, url.hostname), ("http", "127.0.0.1"))
        with socket.socket() as probe:
            probe.settimeout(2)
            self.assertNotEqual(probe.connect_ex(("127.0.0.1", url.port)), 0)


if __name__ == "__main__":
    unittest.main()
