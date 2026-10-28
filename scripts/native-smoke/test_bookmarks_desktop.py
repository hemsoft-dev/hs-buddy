import importlib.util
import os
from pathlib import Path
import socket
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

    def test_the_failure_url_is_a_closed_loopback_port(self):
        url = urlparse(desktop.unreachable_url())
        self.assertEqual((url.scheme, url.hostname), ("http", "127.0.0.1"))
        with socket.socket() as probe:
            probe.settimeout(2)
            self.assertNotEqual(probe.connect_ex(("127.0.0.1", url.port)), 0)


if __name__ == "__main__":
    unittest.main()
