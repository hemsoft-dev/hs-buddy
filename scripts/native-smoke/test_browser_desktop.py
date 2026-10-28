import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.request

spec = importlib.util.spec_from_file_location("browser_desktop", Path(__file__).with_name("browser-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def fetch(url, cookie=None):
    request = urllib.request.Request(url, headers={"Cookie": cookie} if cookie else {})
    with urllib.request.urlopen(request, timeout=5) as response:
        return response.status, dict(response.headers), response.read().decode("utf-8")


class BrowserDesktopTests(unittest.TestCase):
    def test_the_x11_shortcuts_are_pressed_with_the_pointer_over_a_page(self):
        # #880: Ctrl+Tab, Ctrl+Shift+Tab and Ctrl+F4 (and Ctrl+Q ending the
        # drive) are sent only after the pointer rests over the page.
        class Driver:
            def drive(self, name, actions, record=False):
                self.actions = actions
                return Path("flow")
        driver = Driver()
        with patch.object(desktop.github, "check"):
            desktop.shortcut_pass(driver)
        kinds = [action[0] for action in driver.actions]
        hovered = kinds.index("hover")
        shortcuts = [action[1] for action in driver.actions if action[0] == "shortcut"]
        self.assertEqual(shortcuts, ["Tab", "shift+Tab", "F4"])
        self.assertLess(hovered, kinds.index("shortcut"))
        self.assertNotIn("click", kinds[hovered:])

    def test_non_runner_is_refused_before_argument_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()

    def test_the_site_counts_visits_in_a_cookie_and_records_the_probe(self):
        with desktop.Site() as site:
            self.assertTrue(site.origin.startswith("http://127.0.0.1:"))
            status, headers, body = fetch(site.origin + "ok")
            self.assertEqual(status, 200)
            self.assertIn("Visit 1", body)
            self.assertIn(desktop.PAGE_TEXT, body)
            self.assertIn("visits=1", headers["Set-Cookie"])
            _, _, body = fetch(site.origin + "ok", cookie="other=x; visits=4")
            self.assertIn("Visit 5", body)

            _, _, probe = fetch(site.origin + "probe")
            self.assertIn(f"127.0.0.2:{site.server.server_port}/private", probe)
            self.assertIn(f"{desktop.frame_origin(site.server.server_port)}/frame", probe)
            self.assertNotIn("PORT", probe)
            fetch(site.origin + "beacon?name=popup&value=blocked")
            self.assertEqual(site.beacons, {"popup": "blocked"})

            _, headers, _ = fetch(site.origin + "download")
            self.assertIn("attachment", headers["Content-Disposition"])
            self.assertEqual(site.downloads, 1)

    def test_bookmarks_point_at_the_fixture_and_end_on_the_page(self):
        items = desktop.bookmarks("http://127.0.0.1:9/")
        self.assertEqual([item["url"] for item in items], ["http://127.0.0.1:9/probe", "http://127.0.0.1:9/ok"])
        self.assertEqual({item["category"] for item in items}, {"Fixture"})

    def test_audit_records_keep_only_the_origin(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "audit.jsonl"
            self.assertEqual(desktop.audit_records(path), [])
            path.write_text("\n".join(json.dumps(record) for record in [
                dict(kind="internal", origin="http://127.0.0.2:5000", reason=""),
                dict(kind="download", origin="http://127.0.0.1:5000", reason=""),
            ]) + "\n", encoding="utf-8")
            self.assertEqual(desktop.audit_records(path),
                [("internal", "http://127.0.0.2:5000"), ("download", "http://127.0.0.1:5000")])
            path.write_text(json.dumps(dict(kind="popup", origin="https://a.example/secret", reason="")),
                encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "Unexpected audit record"):
                desktop.audit_records(path)

    def test_evaluation_needs_the_required_refusals_and_nothing_granted(self):
        good = dict(popup="blocked", geolocation="denied-1", camera="denied-NotAllowedError")
        frame = desktop.frame_origin(5000)
        records = [("internal", "http://127.0.0.2:5000"), ("internal", frame),
            ("download", "http://127.0.0.1:5000"), ("permission", "http://127.0.0.1:5000")]
        result = desktop.evaluate(records, good, 1, frame)
        self.assertEqual(result["refusals"], ["download", "internal", "permission"])
        with self.assertRaisesRegex(RuntimeError, "download"):
            desktop.evaluate([("internal", frame)], good, 1, frame)
        with self.assertRaisesRegex(RuntimeError, "iframe"):
            desktop.evaluate(records[:1] + records[2:], good, 1, frame)
        with self.assertRaisesRegex(RuntimeError, "popup"):
            desktop.evaluate(records, dict(good, popup="opened"), 1, frame)
        with self.assertRaisesRegex(RuntimeError, "granted"):
            desktop.evaluate(records, dict(good, camera="granted"), 1, frame)
        with self.assertRaisesRegex(RuntimeError, "never requested"):
            desktop.evaluate(records, good, 0, frame)


if __name__ == "__main__":
    unittest.main()
