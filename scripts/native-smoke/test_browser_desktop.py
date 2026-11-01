import importlib.util
import base64
import json
import os
from pathlib import Path
import tempfile
import struct
import subprocess
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
    def test_linux_capture_recovers_missing_chrome_and_titles_from_measured_bands(self):
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('browser-linux-workspace-band.json').read_text())
        ocr = desktop.settings.ocr
        pixels = base64.b64decode(fixture['pixelsBase64'])
        with self.assertRaisesRegex(RuntimeError, 'menu row'):
            ocr.workspace_words(fixture['sparseWords'])
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / 'capture.png'
            def header(scale=1):
                return b'\x89PNG\r\n\x1a\n' + b'\0' * 8 + struct.pack('>II', fixture['width']*scale, fixture['height']*scale)
            for scale in (1, 2):
                image.write_bytes(header(scale))
                scaled = b''.join(pixels[y:y+15]*scale for y in range(0, len(pixels), 15))
                with patch.object(ocr.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, scaled)):
                    self.assertEqual(ocr.workspace_geometry(image), tuple(value*scale for value in fixture['band']))
            image.write_bytes(header())
            with patch.object(ocr.sys, 'platform', 'linux'), \
                    patch.object(ocr, 'recognize', side_effect=[fixture['sparseWords'], fixture['bandWords']]) as recognize, \
                    patch.object(ocr.subprocess, 'run', side_effect=[subprocess.CompletedProcess([], 0, pixels), subprocess.CompletedProcess([], 0, b'')]) as run:
                self.assertEqual(ocr.locate_image(image, 'Bookmarks: All', workspace=True),
                    dict(left=193, top=35, width=92, height=28))
                self.assertTrue(all(call.args[1] == 11 for call in recognize.call_args_list))
                self.assertIn('crop=iw:34:0:34:exact=1', run.call_args_list[1].args[0])
            for invalid in (pixels[:-1], bytes(len(pixels)), pixels[20*15:] + bytes(20*15)):
                with patch.object(ocr.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, invalid)):
                    with self.assertRaises(RuntimeError):
                        ocr.workspace_geometry(image)
        # Cropping does not permit choosing between duplicate tab titles.
        duplicate = [dict(word, left=word['left']+400, line='duplicate') for word in fixture['bandWords']]
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.locate_label(fixture['bandWords'] + duplicate, 'Bookmarks: All')

    def test_windows_workspace_uses_native_geometry_and_the_workspace_command(self):
        from unittest.mock import Mock
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('browser-windows-workspace-title.json').read_text())
        ocr = desktop.settings.ocr
        with self.assertRaisesRegex(RuntimeError, 'menu row'):
            ocr.workspace_words(fixture['sparseWords'])
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / 'capture.png'
            image.write_bytes(b'owned capture')
            with patch.object(ocr.sys, 'platform', 'win32'), \
                    patch.object(ocr, 'recognize_windows', return_value=fixture['windowsWords']) as native, \
                    patch.object(ocr, 'recognize', side_effect=AssertionError('Mixed target engines')):
                for _ in range(2):
                    self.assertEqual(ocr.locate_image(image, fixture['target'], workspace=True),
                        dict(left=189, top=45, width=76, height=9))
                self.assertEqual(native.call_count, 2)
            driver = object.__new__(desktop.BrowserDriver)
            driver.session, driver.profile = 'windows', Path(root)
            driver.config, driver.executable, driver.checkout = image, Path(root) / 'owned.exe', Path(root)
            driver.large_size = (1044, 788)
            driver.backend = Mock(url='http://127.0.0.1:9')
            driver.input_desktop = False
            with patch.object(desktop.settings, 'run') as execute:
                driver.drive('workspace', [('workspace', fixture['target'])])
            args = execute.call_args.args
            self.assertIn('workspace64:' + desktop.settings.b64(fixture['target']), args[args.index('-Actions') + 1].split(','))

    def test_real_mac_workspace_title_is_exact_and_excludes_page_footer_and_menu(self):
        from unittest.mock import Mock
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('browser-macos-workspace-title.json').read_text())
        ocr = desktop.settings.ocr
        with self.assertRaises(RuntimeError):
            ocr.locate_label(fixture['nativeWords'], fixture['target'])
        expected = dict(left=182, top=44, width=78, height=9)
        words = fixture['words']
        # The same title in page or footer text must not compete with a tab.
        copies = [dict(text=fixture['target'], left=182, top=top, width=78, height=9, line=str(top))
            for top in (200, 808)]
        self.assertEqual(ocr.locate_label(ocr.workspace_words(words + copies), fixture['target']), expected)
        row_id = next(word['line'] for word in words if word['text'] == 'Bookmarks:')
        duplicate = [dict(word, left=word['left'] + 400, line='duplicate') for word in words if word['line'] == row_id]
        with self.assertRaises(ocr.AmbiguousLabel):
            ocr.locate_label(ocr.workspace_words(words + duplicate), fixture['target'])
        self.assertLess(ocr.locate_label(ocr.workspace_words(words), 'Buddy Probe')['top'], 70)
        for label in ('File', 'PROBE PAGE', 'jobs'):
            with self.assertRaises(RuntimeError):
                ocr.locate_label(ocr.workspace_words(words), label)
        with self.assertRaises(RuntimeError):
            ocr.workspace_words([])
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / 'capture.png'
            window = Mock()
            window.screenshot.side_effect = lambda path: path.write_bytes(b'owned capture')
            driver = object.__new__(desktop.BrowserDriver)
            driver.session = 'macos'
            with patch.object(ocr.sys, 'platform', 'darwin'), \
                    patch.object(ocr, 'recognize', return_value=words) as sparse, \
                    patch.object(ocr, 'recognize_macos', side_effect=AssertionError('Mixed target engines')):
                self.assertEqual(driver.locate(window, image, 'workspace', fixture['target']), expected)
            self.assertEqual(window.screenshot.call_count, 2)
            self.assertTrue(all(call.args[1] == 11 for call in sparse.call_args_list))

    def test_recorded_mac_browser_returns_through_the_global_bookmarks_tab(self):
        words = json.loads(Path(__file__).with_name('fixtures').joinpath('browser-global-bookmarks-words.json').read_text())

        class Driver:
            def drive(self, name, actions, record=False):
                for kind, *values in actions:
                    if values and 'Bookmarks' in str(values[0]):
                        target = desktop.settings.ocr.locate_label(words, values[0], content=kind == 'content')
                        self.target = target
                return Path('flow')

        driver = Driver()
        with patch.object(desktop.github, 'check') as check:
            desktop.first_pass(driver, 'dark')
        self.assertEqual(len(check.call_args_list), 4)
        self.assertTrue(all(call.kwargs.get('browser') for call in check.call_args_list))
        self.assertEqual(driver.target, dict(left=182, top=44, width=78, height=9))

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
        first = kinds.index("shortcut")
        shortcuts = [action[1] for action in driver.actions if action[0] == "shortcut"]
        self.assertEqual(shortcuts, ["Tab", "shift+Tab", "F4"])
        # The last thing before the first shortcut that moves the pointer is a hover.
        moves = [kind for kind in kinds[:first] if kind in ("hover", "click", "workspace")]
        self.assertEqual(moves[-1], "hover")
        self.assertNotIn("click", kinds[first:])
        self.assertNotIn("workspace", kinds[first:])
        self.assertEqual(driver.actions[kinds.index("hover")][1:], ("page",))
        self.assertIn(("tooltip_leave",), driver.actions[:first])
        self.assertNotIn(("hover", "title"), driver.actions)

    def test_tooltip_leave_requires_visible_tooltip_then_unique_tab_title(self):
        from unittest.mock import Mock
        driver = object.__new__(desktop.BrowserDriver)
        driver.locate = Mock(return_value=dict(left=10, top=20, width=80, height=20))
        window = Mock(window="42")
        for results, succeeds in [
                ([desktop.settings.ocr.AmbiguousLabel("two titles"), {}], True),
                ([{}], False),
                ([desktop.settings.ocr.AmbiguousLabel("two titles"),
                  desktop.settings.ocr.AmbiguousLabel("still two titles")], False)]:
            with patch.object(desktop.settings, "run"), patch.object(desktop.time, "sleep"), \
                    patch.object(desktop.settings.ocr, "locate_image", side_effect=results):
                if succeeds:
                    driver.interact(window, Path("flow"), "tooltip_leave", [])
                    window.hover.assert_called_with("page")
                    window.screenshot.assert_any_call(Path("flow/tooltip-on-tab-light.png"))
                    window.screenshot.assert_any_call(Path("flow/tooltip-over-page-light.png"))
                else:
                    with self.assertRaises(RuntimeError):
                        driver.interact(window, Path("flow"), "tooltip_leave", [])

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
