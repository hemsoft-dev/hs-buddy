import importlib.util
import base64
import json
import os
from pathlib import Path
import tempfile
import struct
import subprocess
import unittest
from unittest.mock import Mock, patch
import urllib.request

spec = importlib.util.spec_from_file_location("browser_desktop", Path(__file__).with_name("browser-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def fetch(url, cookie=None):
    request = urllib.request.Request(url, headers={"Cookie": cookie} if cookie else {})
    with urllib.request.urlopen(request, timeout=5) as response:
        return response.status, dict(response.headers), response.read().decode("utf-8")


class BrowserDesktopTests(unittest.TestCase):
    def test_bookmark_activity_requires_exact_explicit_delta_and_unchanged_other_counters(self):
        before = dict(tabsOpened=40, prsViewed=7, appLaunches=5, totalUptimeMs=999)
        backend = Mock()
        # Three bookmark opens and two launches, each with its Dashboard and one closed session.
        expected = dict(before, tabsOpened=45, appLaunches=7, totalUptimeMs=4000)
        backend.query.return_value = expected
        self.assertEqual(desktop.require_bookmark_activity(backend, before, 3, 2), expected)
        backend.query.assert_called_once_with('buddyStats:get')
        for wrong in (dict(expected, tabsOpened=44), dict(expected, tabsOpened=46),
                dict(expected, prsViewed=8), {}):
            backend.query.return_value = wrong
            with self.subTest(wrong=wrong), self.assertRaisesRegex(RuntimeError, 'differs from explicit opens'):
                desktop.require_bookmark_activity(backend, before, 3, 2)
        for wrong in (dict(expected, appLaunches=6), dict(expected, appLaunches=8),
                dict(expected, totalUptimeMs=999), dict(expected, lastSessionStart=1)):
            backend.query.return_value = wrong
            with self.subTest(wrong=wrong), self.assertRaisesRegex(RuntimeError, 'Native sessions differ'):
                desktop.require_bookmark_activity(backend, before, 3, 2)

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
        with patch.object(desktop.github, 'check') as check, \
                patch.object(desktop, 'require_zoomed', return_value={}), \
                patch.object(desktop, 'require_unzoomed', return_value={}), \
                patch.object(desktop, 'require_tree_favicon', return_value={}), \
                patch.object(desktop, 'require_favicon', return_value={}) as favicon:
            desktop.first_pass(driver, 'dark')
        favicon.assert_called_once_with(Path('flow') / 'bookmark-favicons-dark.png')
        # Zoom and unzoom each check the page and the toolbar label separately.
        self.assertEqual(len(check.call_args_list), 10)
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
        # Only the page bookmark has a favicon, served by the fixture site itself.
        self.assertEqual([item.get("faviconUrl") for item in items], [None, "http://127.0.0.1:9/favicon.png"])

    def test_the_site_serves_a_solid_favicon_and_counts_requests(self):
        with desktop.Site() as site:
            request = urllib.request.Request(site.origin + "favicon.png")
            with urllib.request.urlopen(request, timeout=5) as response:
                self.assertEqual(response.headers["Content-Type"], "image/png")
                body = response.read()
            self.assertEqual(body, desktop.FAVICON)
            self.assertEqual(site.favicons, 1)
        self.assertTrue(body.startswith(b"\x89PNG\r\n\x1a\n"))
        self.assertEqual(struct.unpack(">II", body[16:24]), (32, 32))
        # The image data decodes to the one colour.
        import zlib
        data = zlib.decompress(body[body.index(b"IDAT") + 4:body.index(b"IEND") - 8])
        rows = [data[y * (1 + 32 * 3):(y + 1) * (1 + 32 * 3)] for y in range(32)]
        self.assertTrue(all(row == b"\0" + bytes(desktop.FAVICON_RGB) * 32 for row in rows))

    def test_favicon_pixels_are_the_magenta_block(self):
        with patch.object(desktop.colour_blocks, "measure", return_value=(400, None)) as measure:
            self.assertEqual(desktop.favicon_pixels(Path("capture.png")), (400, None))
        measure.assert_called_once_with(Path("capture.png"), "magenta")

    def test_the_fixture_page_paints_a_fixed_size_lime_block(self):
        page = desktop.Site().page(1)
        self.assertIn(desktop.PAGE_BLOCK_CSS, page)
        self.assertIn("#00ff00", desktop.PAGE_BLOCK_CSS)
        self.assertEqual(desktop.PAGE_BLOCK, "lime")

    def test_the_zoom_check_needs_the_page_content_to_scale_one_step(self):
        def block(width, height):
            return (width * height, dict(left=40, top=300, width=width, height=height))
        with tempfile.TemporaryDirectory() as root:
            before, after = Path(root) / "page-dark.png", Path(root) / "browser-page-zoom-dark.png"
            for sizes, error in (((block(160, 80), block(176, 88)), None),
                    ((block(320, 160), block(352, 176)), None),
                    # The toolbar said 110% but the engine kept the page at 100%.
                    ((block(160, 80), block(160, 80)), "did not scale"),
                    ((block(160, 80), block(192, 96)), "did not scale"),
                    ((block(160, 80), (0, None)), "not shown in browser-page-zoom-dark.png"),
                    (((0, None), block(176, 88)), "not shown in page-dark.png")):
                with self.subTest(sizes=sizes), patch.object(desktop.colour_blocks, "measure", side_effect=sizes) as measure:
                    if error:
                        with self.assertRaisesRegex(RuntimeError, error):
                            desktop.require_zoomed(before, after)
                    else:
                        evidence = desktop.require_zoomed(before, after)
                        self.assertEqual(evidence["ratios"], [1.1, 1.1])
                        self.assertEqual(json.loads(after.with_suffix(".zoom.json").read_text())["ratios"], [1.1, 1.1])
                    self.assertEqual(measure.call_args_list[0].args, (before, "lime"))

    def test_the_zoom_out_check_needs_the_page_content_back_at_its_100_percent_size(self):
        def block(width, height):
            return (width * height, dict(left=40, top=300, width=width, height=height))
        with tempfile.TemporaryDirectory() as root:
            before, after = Path(root) / "page-dark.png", Path(root) / "browser-page-unzoom-dark.png"
            for sizes, error in (((block(160, 80), block(160, 80)), None),
                    # A pixel of rounding either way is the same size.
                    ((block(160, 80), block(161, 79)), None),
                    ((block(320, 160), block(318, 162)), None),
                    # The toolbar said 100% but the engine left the page at 110%.
                    ((block(160, 80), block(176, 88)), "did not return to its 100% size"),
                    ((block(160, 80), block(144, 72)), "did not return to its 100% size"),
                    ((block(160, 80), (0, None)), "not shown in browser-page-unzoom-dark.png")):
                with self.subTest(sizes=sizes), patch.object(desktop.colour_blocks, "measure", side_effect=sizes) as measure:
                    if error:
                        with self.assertRaisesRegex(RuntimeError, error):
                            desktop.require_unzoomed(before, after)
                    else:
                        evidence = desktop.require_unzoomed(before, after)
                        self.assertEqual(json.loads(after.with_suffix(".zoom.json").read_text()), evidence)
                    self.assertEqual(measure.call_args_list[1].args[1], "lime")

    def test_the_favicon_check_needs_an_icon_sized_block(self):
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / "capture.png"
            for result, error in (((400, dict(left=1, top=1, width=20, height=20)), None),
                    ((40, dict(left=1, top=1, width=6, height=6)), "no favicon"),
                    ((0, None), "no favicon"),
                    ((90000, dict(left=0, top=0, width=300, height=300)), "more than an icon")):
                with self.subTest(result=result), patch.object(desktop, "favicon_pixels", return_value=result):
                    if error:
                        with self.assertRaisesRegex(RuntimeError, error):
                            desktop.require_favicon(image)
                    else:
                        self.assertEqual(desktop.require_favicon(image)["pixels"], 400)
                        self.assertEqual(json.loads(image.with_suffix(".favicon.json").read_text())["pixels"], 400)

    def test_alt_zoom_changes_the_toolbar_level_then_takes_it_back(self):
        class Driver:
            def drive(self, name, actions, record=False):
                self.actions = actions
                return Path("flow")
        driver = Driver()
        with patch.object(desktop.github, "check") as check, patch.object(desktop, "require_favicon", return_value={}), \
                patch.object(desktop, "require_tree_favicon", return_value={}), \
                patch.object(desktop, "require_zoomed", return_value={}) as zoomed, \
                patch.object(desktop, "require_unzoomed", return_value={}) as unzoomed:
            desktop.first_pass(driver, "dark")
        actions = driver.actions
        zoom_in = actions.index(("altkey", "="))
        # The page is open first; then the level changes, is captured, and is taken back.
        self.assertLess(actions.index(("wait_text", "Visit 1")), zoom_in)
        self.assertEqual(actions[zoom_in:zoom_in + 8], [("altkey", "="), ("wait_text", "110%"), ("sleep", 1),
            ("capture", "browser-page-zoom-dark"), ("altkey", "-"), ("wait_text", "100%"), ("sleep", 1),
            ("capture", "browser-page-unzoom-dark")])
        # The favicon loads after the list: its colour is waited for before the capture.
        favicon = actions.index(("capture", "bookmark-favicons-dark"))
        self.assertEqual(actions[favicon - 1], ("wait_colour", "magenta", desktop.FAVICON_MIN_PIXELS))
        self.assertLess(favicon, actions.index(("content", "Fixture page")))
        checked = {call.args[0].name: call.args[1:] for call in check.call_args_list}
        every = [(call.args[0].name, call.args[1:]) for call in check.call_args_list]
        # The page heading and the toolbar label are required separately on each capture.
        self.assertEqual([args for name, args in every if name == "browser-page-zoom-dark.png"],
            [(desktop.PAGE_TEXT,), ("110%",)])
        # The page itself must have grown, not just the toolbar's number.
        zoomed.assert_called_once_with(Path("flow") / "page-dark.png", Path("flow") / "browser-page-zoom-dark.png")
        # And Alt+- must have shrunk it back to its 100% size.
        self.assertEqual([args for name, args in every if name == "browser-page-unzoom-dark.png"],
            [(desktop.PAGE_TEXT,), ("100%",)])
        unzoomed.assert_called_once_with(Path("flow") / "page-dark.png", Path("flow") / "browser-page-unzoom-dark.png")

    def test_both_passes_expand_the_tree_after_every_card_click_and_check_its_row(self):
        class Driver:
            def drive(self, name, actions, record=False):
                self.actions, self.record = actions, record
                return Path("flow")
        for theme, run in (("dark", lambda driver: desktop.first_pass(driver, "dark")),
                ("light", lambda driver: desktop.light_pass(driver, 3))):
            with self.subTest(theme=theme):
                driver = Driver()
                with patch.object(desktop.github, "check") as check, \
                        patch.object(desktop, "require_favicon", return_value={}), \
                        patch.object(desktop, "require_tree_favicon", return_value=dict(tree={}, card={})) as tree, \
                        patch.object(desktop, "require_zoomed", return_value={}), \
                        patch.object(desktop, "require_unzoomed", return_value={}):
                    result = run(driver)
                actions, steps = driver.actions, desktop.tree_favicon_steps(theme)
                self.assertTrue(driver.record)
                start = next(index for index in range(len(actions)) if actions[index:index + len(steps)] == steps)
                # The category is chosen in the tree, between the toolbar and the
                # search label, where no card's "Fixture" category is; then the
                # drive waits, bounded, for the row's favicon beside the card's.
                self.assertEqual(actions[start + 2], ("between", "Fixture", "Refresh", "Search"))
                self.assertEqual(actions[start + 3], ("wait_colour", "magenta", desktop.TREE_FAVICON_MIN_PIXELS, 2))
                self.assertEqual(actions[start + 4], ("capture", f"bookmark-tree-favicons-{theme}"))
                # Once the row shows, "Fixture page" is only clicked with the
                # card searched away, so the click is the tree row's. The dark
                # drive ends back on the page the restart restores; the light
                # one on the probe's tab.
                after = actions[start + len(steps):]
                self.assertEqual(after, [("box", "Search", "Probe"), ("sleep", 1), ("content", "Fixture page"),
                    ("wait_text", "Visit 1")] if theme == "dark"
                    else [("workspace", "Buddy Probe"), ("wait_text", desktop.BLOCKED_TEXT), ("capture", "blocked-light")])
                self.assertNotIn(("content", "Fixture page"), actions[start:start + len(steps)])
                checked = {call.args[0].name: call.args[1:] for call in check.call_args_list}
                self.assertEqual(checked[f"bookmark-tree-favicons-{theme}.png"], ("In Fixture", "Fixture page", "Probe page"))
                tree.assert_called_once_with(Path("flow") / f"bookmark-tree-favicons-{theme}.png")
                self.assertEqual(result["tree_favicon"], dict(tree={}, card={}))

    def test_the_tree_check_needs_the_rows_smaller_icon_left_of_the_cards(self):
        def block(left, size, pixels=None):
            return dict(pixels=pixels or size * size, left=left, top=200, width=size, height=size)
        tree, card = block(90, 12), block(333, 20)
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / "bookmark-tree-favicons-dark.png"
            for blocks, error in (([tree, card], None),
                    # Only the card shows it: the tree row lost its favicon.
                    ([card], "do not both show the favicon"),
                    ([], "do not both show the favicon"),
                    # The left block is no smaller than the card's, so not the row's.
                    ([block(90, 20), card], "not the tree row's smaller icon"),
                    ([card, block(600, 12)], "not the tree row's smaller icon"),
                    ([tree, block(333, 200)], "more than an icon"),
                    ([tree, card, block(700, 20)], "do not both show the favicon")):
                with self.subTest(blocks=blocks), \
                        patch.object(desktop.colour_blocks, "measure_blocks", return_value=blocks) as measured:
                    if error:
                        with self.assertRaisesRegex(RuntimeError, error):
                            desktop.require_tree_favicon(image)
                    else:
                        self.assertEqual(desktop.require_tree_favicon(image), dict(tree=tree, card=card))
                    self.assertEqual(measured.call_args.args, (image, "magenta"))
                    self.assertEqual(json.loads(image.with_suffix(".favicon.json").read_text())["blocks"], blocks)
        # Blocks under the row icon's share of pixels are not counted at all.
        with patch.object(desktop.colour_blocks, "measure_blocks",
                return_value=[block(90, 12, desktop.TREE_FAVICON_MIN_PIXELS - 1), card]):
            with tempfile.TemporaryDirectory() as root, self.assertRaisesRegex(RuntimeError, r"\(1 magenta blocks\)"):
                desktop.require_tree_favicon(Path(root) / "capture.png")

    def test_each_tree_category_open_is_counted_with_the_bookmark_opens(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            config = root / "dark" / "config.json"
            config.parent.mkdir()
            config.write_text(json.dumps(dict(native={}, ui={})), encoding="utf-8")
            template = root / "light.json"
            template.write_text(json.dumps(dict(ui={})), encoding="utf-8")
            site = Mock(origin="http://127.0.0.1:1/", favicons=1, beacons={}, downloads=1)
            site.lock = __import__("threading").Lock()
            site.server.server_port = 1
            activity = Mock(return_value={})
            with patch.object(desktop.github, "installed_binary", return_value={}), \
                    patch.object(desktop, "fixture", side_effect=[config, template]), \
                    patch.object(desktop.terminal, "write_config"), patch.object(desktop, "seed"), \
                    patch.object(desktop, "BrowserDriver", return_value=Mock(large_size=(1200, 820))), \
                    patch.object(desktop, "saved_zoom", return_value=None), \
                    patch.object(desktop, "restore_zoom"), patch.object(desktop, "first_pass", return_value=dict(favicon={})), \
                    patch.object(desktop, "restart_pass"), patch.object(desktop, "light_pass", return_value={}), \
                    patch.object(desktop, "require_bookmark_activity", activity), \
                    patch.object(desktop, "audit_records", return_value=[]), patch.object(desktop, "evaluate", return_value={}), \
                    patch.object(desktop, "saved_tabs", return_value=[item["url"] for item in desktop.bookmarks(site.origin)]), \
                    patch.dict(os.environ):
                desktop.qualify(Path("buddy"), root, "windows", "1.0.0", Mock(query=Mock(return_value={})), site)
        # Three bookmark opens and a tree category open in the first pass, the
        # card again and the category again in the light pass.
        self.assertEqual([call.args[2:] for call in activity.call_args_list], [(4, 1), (4, 2), (6, 3)])

    def test_the_light_pass_repeats_the_favicon_and_zoom_steps_on_a_recording(self):
        class Driver:
            def drive(self, name, actions, record=False):
                self.name, self.actions, self.record = name, actions, record
                return Path("flow")
        driver = Driver()
        with patch.object(desktop.github, "check") as check, \
                patch.object(desktop, "require_favicon", return_value={}) as favicon, \
                patch.object(desktop, "require_tree_favicon", return_value={}), \
                patch.object(desktop, "require_zoomed", return_value={}) as zoomed, \
                patch.object(desktop, "require_unzoomed", return_value={}) as unzoomed:
            desktop.light_pass(driver, 3)
        actions = driver.actions
        # The keyboard zoom flow is recorded with the rest of the light drive.
        self.assertEqual((driver.name, driver.record), ("browser-light", True))
        # The page at 100% comes first, so the zoom checks compare against it.
        self.assertEqual(actions[:2], [("wait_text", "Visit 3"), ("capture", "page-light")])
        favicon_at = actions.index(("workspace", "Bookmarks: All"))
        self.assertEqual(actions[favicon_at:favicon_at + 4], desktop.favicon_steps("light"))
        # Opening the card again selects its tab without a new visit and
        # leaves the keyboard in the browser for the Alt keys.
        reopen = actions.index(("content", "Fixture page"))
        self.assertGreater(reopen, favicon_at)
        self.assertEqual(actions[reopen + 1], ("wait_text", "Visit 3"))
        self.assertEqual(actions[reopen + 2:reopen + 10], desktop.zoom_steps("light"))
        self.assertEqual([action for action in actions if action[0] == "content"], [("content", "Fixture page")])
        # It still ends on the probe's tab, where the X11 shortcut pass starts.
        self.assertEqual(actions[-1], ("capture", "blocked-light"))
        checked = {call.args[0].name: call.args[1:] for call in check.call_args_list}
        self.assertEqual(checked["bookmark-favicons-light.png"], ("Fixture page", "Probe page"))
        every = [(call.args[0].name, call.args[1:]) for call in check.call_args_list]
        # The page heading and the toolbar label are required separately on each capture.
        self.assertEqual([args for name, args in every if name == "browser-page-zoom-light.png"],
            [(desktop.PAGE_TEXT,), ("110%",)])
        self.assertEqual([args for name, args in every if name == "browser-page-unzoom-light.png"],
            [(desktop.PAGE_TEXT,), ("100%",)])
        favicon.assert_called_once_with(Path("flow") / "bookmark-favicons-light.png")
        zoomed.assert_called_once_with(Path("flow") / "page-light.png", Path("flow") / "browser-page-zoom-light.png")
        unzoomed.assert_called_once_with(Path("flow") / "page-light.png", Path("flow") / "browser-page-unzoom-light.png")

    def test_the_saved_page_zoom_is_restored(self):
        with tempfile.TemporaryDirectory() as root:
            config = Path(root) / "config.json"
            config.write_text(json.dumps(dict(native=dict(browser=dict(tabs=[], zoom=1.0)))), encoding="utf-8")
            self.assertEqual(desktop.saved_zoom(config), 1.0)
            desktop.terminal.write_config(config, lambda value: value["native"]["browser"].update(zoom=1.1))
            desktop.restore_zoom(config, 1.0)
            self.assertEqual(desktop.saved_zoom(config), 1.0)
            # Nothing was saved before: the key goes again, and the tabs stay.
            desktop.restore_zoom(config, None)
            self.assertEqual(json.loads(config.read_text())["native"]["browser"], dict(tabs=[]))
            config.write_text(json.dumps(dict(native={})), encoding="utf-8")
            self.assertIsNone(desktop.saved_zoom(config))
            desktop.restore_zoom(config, None)
            self.assertEqual(json.loads(config.read_text()), dict(native={}))

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
