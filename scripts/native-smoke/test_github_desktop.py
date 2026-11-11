"""Bind GitHub evidence to the installed binary and its actual route content."""
import hashlib
import base64
import importlib.util
import json
import struct
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("github_desktop", Path(__file__).with_name("github-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class InstalledEvidence(unittest.TestCase):
    def test_restarted_qualification_requires_the_actual_needs_review_fixture_row(self):
        capture = json.loads(Path(__file__).with_name('fixtures').joinpath('github-restored-needs-review.json').read_text())
        original_check = desktop.check
        class CheckedRestoredList(Exception):
            pass
        for missing in (None, 'Needs Review', '#8 Paginated fixture follow-up'):
            with self.subTest(missing=missing), TemporaryDirectory() as temporary:
                output = Path(temporary) / 'profile'
                def drive(name, *args, **kwargs):
                    if name == 'review-dark':
                        config = output / 'config.json'
                        value = json.loads(config.read_text())
                        value['native']['github'] = dict(viewModes={'pr-list-my-prs': 'list'})
                        config.write_text(json.dumps(value))
                    directory = output / name
                    directory.mkdir()
                    return directory
                checked = []
                def check(path, *markers, **kwargs):
                    checked.append(path.name)
                    if path.name != 'restored-sidebar-list-dark.png':
                        return
                    original_check(path, *markers, **kwargs)
                    raise CheckedRestoredList
                def run(*args, **kwargs):
                    if args[0] == 'ffmpeg':
                        return ''
                    mode = int(args[args.index('--psm') + 1])
                    text = next(layout['text'] for layout in capture['layouts']
                        if layout['image'] == Path(args[1]).name and layout['mode'] == mode)
                    return text.replace(missing.casefold(), '') if missing else text
                driver = Mock()
                driver.drive.side_effect = drive
                backend = Mock()
                backend.query.return_value = dict(viewModes={'pr-list-my-prs': 'list'})
                api = Mock(requests=[dict(method='POST', path='/issues/7/comments', account='fixture-one',
                    body=dict(body='Installed native fixture comment'))])
                with patch.object(desktop, 'installed_binary', return_value={}), \
                        patch.object(desktop, 'mutate'), patch.object(desktop, 'verify_detail_requests'), \
                        patch.object(desktop, 'require_activity'), patch.object(desktop.settings, 'Driver', return_value=driver), \
                        patch.object(desktop, 'check', side_effect=check), patch.object(desktop.settings, 'run', side_effect=run):
                    expected = RuntimeError if missing else CheckedRestoredList
                    with self.assertRaises(expected):
                        desktop.qualify(Path('owned-buddy'), output, 'windows', 'fixture', backend, api)
                self.assertEqual(checked[-1], 'restored-sidebar-list-dark.png')
                self.assertEqual(driver.drive.call_count, 2)

    def test_activity_requires_exact_deltas_and_does_not_reset_existing_totals(self):
        before = dict(tabsOpened=12, prsViewed=30, prsReviewed=4)
        backend = Mock()
        backend.query.return_value = dict(tabsOpened=17, prsViewed=32, prsReviewed=5)
        desktop.require_activity(backend, before, dict(tabsOpened=5, prsViewed=2, prsReviewed=1))
        backend.query.assert_called_once_with("buddyStats:get")
        for incorrect in (dict(tabsOpened=16, prsViewed=32, prsReviewed=5),
                dict(tabsOpened=18, prsViewed=32, prsReviewed=5), {}):
            backend.query.return_value = incorrect
            with self.assertRaisesRegex(RuntimeError, "differs from explicit opens"):
                desktop.require_activity(backend, before, dict(tabsOpened=5, prsViewed=2, prsReviewed=1))

    def test_recorded_browser_needs_one_complete_mixed_contrast_layout(self):
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('browser-mixed-contrast.json').read_text(encoding='utf-8'))
        calls = []
        def recognize(*args, **kwargs):
            calls.append(args)
            if args[0] == 'ffmpeg': return ''
            mode = int(args[args.index('--psm') + 1])
            if str(args[1]).endswith('.ocr-scale3-inverted.png'):
                return fixture['invertedComplete'] if mode == 6 else fixture['original'][0]
            offset = 3 if str(args[1]).endswith('.ocr-scale3.png') else 0
            return fixture['original'][offset + (11, 3, 6).index(mode)]
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, 'run', side_effect=recognize):
            image = Path(temporary) / 'blocked.png'
            image.write_bytes(b'untouched original')
            desktop.check(image, 'PROBE PAGE', 'Downloads are blocked', 'Bookmarks', 'Buddy Probe')
            self.assertEqual(image.read_bytes(), b'untouched original')
            self.assertIn('probe page', image.with_suffix('.ocr.txt').read_text(encoding='utf-8'))
        self.assertIn('scale=iw*3:ih*3:flags=neighbor,format=gray,negate', [a[a.index('-vf')+1] for a in calls if a[0]=='ffmpeg'])

    def test_inverted_layout_cannot_hide_forbidden_account_data(self):
        def recognize(*args, **kwargs):
            if args[0] == 'ffmpeg': return ''
            return 'First private Second private' if str(args[1]).endswith('.ocr-scale3-inverted.png') else 'missing'
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, 'run', side_effect=recognize):
            with self.assertRaisesRegex(RuntimeError, 'Another account'):
                desktop.check(Path(temporary) / 'mixed.png', 'First private', absent=('Second private',))

    def test_recorded_ubuntu_page_keeps_its_title_and_page_in_one_layout(self):
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('browser-mixed-contrast.json').read_text(encoding='utf-8'))['ubuntu']
        def recognize(*args, **kwargs):
            if args[0] == 'ffmpeg': return ''
            mode = int(args[args.index('--psm') + 1])
            if str(args[1]).endswith('.ocr-scale3-inverted.png'):
                return fixture['invertedComplete'] if mode == 11 else fixture['original'][0]
            offset = 3 if str(args[1]).endswith('.ocr-scale3.png') else 0
            return fixture['original'][offset + (11, 3, 6).index(mode)]
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, 'run', side_effect=recognize):
            desktop.check(Path(temporary) / 'page.png', 'FIXTURE PAGE', 'Visit 1', 'Buddy Fixture OK')

    def browser_footer_fixture(self):
        return json.loads(Path(__file__).with_name('fixtures').joinpath('browser-status-footer.json').read_text(encoding='utf-8'))

    def replay_browser_footer(self, temporary, *, missing_page=False, missing_footer=False):
        fixture = self.browser_footer_fixture()
        image = Path(temporary) / 'blocked-dark.png'
        image.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\0' * 8 + struct.pack('>II', fixture['width'], fixture['height']))
        crops = []
        def run(*args, **kwargs):
            if args[0] == 'ffmpeg':
                source = Path(args[args.index('-i') + 1])
                width, height = struct.unpack('>II', source.read_bytes()[16:24])
                filters = args[args.index('-vf') + 1]
                if filters.startswith('crop='):
                    crops.append(filters)
                    height = int(filters.split(':')[1])
                else:
                    width *= 3; height *= 3
                Path(args[-1]).write_bytes(b'\x89PNG\r\n\x1a\n' + b'\0' * 8 + struct.pack('>II', width, height))
                return ''
            name, mode = Path(args[1]).name, int(args[args.index('--psm') + 1])
            if '.browser-ocr' in name:
                text = next((attempt['text'] for attempt in fixture['scoped'] if attempt['image'] == name and attempt['mode'] == mode), '')
                return text.replace('PROBE PAGE', '') if missing_page else text
            offset = 6 if 'inverted' in name else 3 if 'scale3' in name else 0
            return fixture['original'][offset + (11, 3, 6).index(mode)]
        def recognize(path, mode):
            if missing_footer: return []
            return next((layout['words'] for layout in fixture['footerLayouts'] if layout['image'] == path.name and layout['mode'] == mode), [])
        return image, crops, run, recognize

    @patch.object(desktop.sys, "platform", "win32")
    def test_real_windows_browser_capture_needs_footer_excluded_from_one_layout(self):
        with TemporaryDirectory() as temporary:
            image, crops, run, recognize = self.replay_browser_footer(temporary)
            original = image.read_bytes()
            with patch.object(desktop.settings, 'run', side_effect=run), patch.object(desktop.settings.ocr, 'recognize', side_effect=recognize):
                with self.assertRaisesRegex(RuntimeError, 'one OCR layout'):
                    desktop.check(image, 'PROBE PAGE', 'Downloads are blocked', 'Bookmarks', 'Buddy Probe')
                desktop.check(image, 'PROBE PAGE', 'Downloads are blocked', 'Bookmarks', 'Buddy Probe', browser=True)
            self.assertEqual(crops, ['crop=iw:759:0:0:exact=1'])
            self.assertEqual(image.read_bytes(), original)
            self.assertNotIn('schedules', image.with_suffix('.ocr.txt').read_text())

    @patch.object(desktop.sys, "platform", "win32")
    def test_browser_footer_scope_cannot_supply_missing_page_text(self):
        with TemporaryDirectory() as temporary:
            image, _, run, recognize = self.replay_browser_footer(temporary, missing_page=True)
            with patch.object(desktop.settings, 'run', side_effect=run), patch.object(desktop.settings.ocr, 'recognize', side_effect=recognize):
                with self.assertRaisesRegex(RuntimeError, 'one OCR layout'):
                    desktop.check(image, 'PROBE PAGE', 'Downloads are blocked', 'Bookmarks', 'Buddy Probe', browser=True)

    @patch.object(desktop.sys, "platform", "win32")
    def test_browser_scope_requires_a_visible_footer_and_propagates_ambiguity(self):
        with TemporaryDirectory() as temporary:
            image, _, run, recognize = self.replay_browser_footer(temporary, missing_footer=True)
            with patch.object(desktop.settings, 'run', side_effect=run), patch.object(desktop.settings.ocr, 'recognize', side_effect=recognize):
                with self.assertRaisesRegex(RuntimeError, 'no unique native status footer'):
                    desktop.check(image, 'PROBE PAGE', browser=True)
            with patch.object(desktop.settings.ocr, 'recognize', side_effect=desktop.settings.ocr.AmbiguousLabel('competing footer')) as recognize:
                with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
                    desktop.check(image, 'PROBE PAGE', browser=True)
                self.assertEqual(recognize.call_count, 1)

    def refresh_fixture(self):
        return json.loads(Path(__file__).with_name('fixtures').joinpath('github-refresh-acknowledgement.json').read_text())

    def review_actions(self):
        class Captured(Exception):
            pass
        driver = Mock()
        driver.drive.side_effect = Captured
        with TemporaryDirectory() as temporary, \
                patch.object(desktop, 'installed_binary', return_value={}), \
                patch.object(desktop, 'mutate'), \
                patch.object(desktop.settings, 'Driver', return_value=driver):
            with self.assertRaises(Captured):
                desktop.qualify(Path('owned-buddy'), Path(temporary) / 'profile', 'macos', 'fixture', Mock(), Mock())
        return driver.drive.call_args.args[1]

    def review_refresh_action(self):
        return self.review_actions()[-1]

    def test_actual_mode_clicks_select_sidebar_when_current_heading_repeats_the_label(self):
        words = json.loads(Path(__file__).with_name('fixtures').joinpath('github-pr-mode-sidebar.json').read_text())['words']
        minimal = [word for word in words if word['text'] in ('File', 'View', 'Help', 'Dashboard', 'Back', 'My', 'PRs')
            and word['top'] < 300]
        with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
            desktop.settings.ocr.locate_label(minimal, 'My PRs', content=True)
        actions = [action for action in self.review_actions() if action[1:] == ('My PRs',)]
        self.assertEqual(len(actions), 2)
        for action in actions:
            with patch.object(desktop.settings.ocr.sys, 'platform', 'linux'), \
                    patch.object(desktop.settings.ocr, 'recognize', return_value=words):
                target = desktop.settings.ocr.locate_image(Path('captured.png'), action[1],
                    content=action[0] == 'content', sidebar=action[0] == 'sidebar') if action[0] == 'sidebar' else \
                    desktop.settings.ocr.locate_label(words, action[1], content=action[0] == 'content')
            self.assertEqual(target, dict(left=109, top=87, width=49, height=13))

    def test_sidebar_scope_uses_measured_geometry_on_each_platform_and_scale(self):
        original = json.loads(Path(__file__).with_name('fixtures').joinpath('github-pr-mode-sidebar.json').read_text())['words']
        for platform in ('linux', 'darwin', 'win32'):
            for scale in (1, 1.25, 2):
                words = [dict(word, **{key: word[key] * scale for key in ('left', 'top', 'width', 'height')}) for word in original]
                with self.subTest(platform=platform, scale=scale), \
                        patch.object(desktop.settings.ocr.sys, 'platform', platform), \
                        patch.object(desktop.settings.ocr, 'recognize', return_value=words), \
                        patch.object(desktop.settings.ocr, 'recognize_windows', return_value=words):
                    target = desktop.settings.ocr.locate_image(Path('captured.png'), 'My PRs', sidebar=True)
                    self.assertEqual(target, {key: value * scale for key, value in
                        dict(left=109, top=87, width=49, height=13).items()})

    def test_sidebar_scope_works_when_windows_ocr_omits_disabled_back(self):
        words = json.loads(Path(__file__).with_name('fixtures').joinpath('github-windows-disabled-back.json').read_text())['words']
        self.assertNotIn('Back', [word['text'] for word in words])
        target = desktop.settings.ocr.sidebar_label(words, 'Needs Review')
        self.assertEqual(target, dict(left=118, top=124, width=86, height=10))

    def test_mac_sidebar_uses_captured_chrome_bands_when_file_is_misread(self):
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('github-macos-sidebar-chrome.json').read_text())
        words, pixels = fixture['words'], base64.b64decode(fixture['pixelsBase64'])
        ocr = desktop.settings.ocr
        with self.assertRaisesRegex(RuntimeError, 'menu row'):
            ocr.chrome_rows(words)
        target = [word for word in words if word['line'] == '11/1/1']
        boundary = next(word for word in words if word['text'] == 'Repositories')
        readable = [dict(word, text='File') if word['text'] == 'Fie' else word for word in words]
        duplicate_menu = readable + [dict(word, top=word['top']+30, line='duplicate-menu')
            for word in readable if word['text'] in ('File', 'view', 'Help')]
        cases = [
            ('captured', words, pixels, None),
            ('missing target', [word for word in words if word not in target], pixels, RuntimeError),
            ('duplicate target', words + [dict(word, top=word['top']+18, line='duplicate') for word in target], pixels, ocr.AmbiguousLabel),
            ('missing boundary', [word for word in words if word != boundary], pixels, RuntimeError),
            ('duplicate boundary', words + [dict(boundary, top=boundary['top']+35, line='duplicate')], pixels, ocr.AmbiguousLabel),
            ('invalid chrome', words, bytes(len(pixels)), RuntimeError),
            ('duplicate chrome', duplicate_menu, pixels, ocr.AmbiguousLabel),
        ]
        for name, measured, bands, error in cases:
            with self.subTest(name=name), TemporaryDirectory() as temporary:
                image = Path(temporary) / 'capture.png'
                image.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\0' * 8 + struct.pack('>II', fixture['width'], fixture['height']))
                driver = object.__new__(desktop.settings.Driver)
                driver.session = 'macos'
                with patch.object(ocr.sys, 'platform', 'darwin'), \
                        patch.object(ocr, 'recognize', return_value=measured) as recognize, \
                        patch.object(ocr.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, bands)), \
                        patch.object(desktop.settings, 'record_measurement') as receipt:
                    if error:
                        with self.assertRaises(error):
                            driver.locate(Mock(), image, 'sidebar', 'Needs Review')
                        receipt.assert_not_called()
                    else:
                        self.assertEqual(driver.locate(Mock(), image, 'sidebar', 'Needs Review'),
                            dict(left=108, top=121, width=89, height=11))
                        self.assertEqual(recognize.call_count, 2)
                        self.assertTrue(all(call.args[1] == 11 for call in recognize.call_args_list))
                        self.assertEqual(receipt.call_count, 2)

    def test_mac_sidebar_measures_clear_chrome_after_all_four_modes_open_tabs(self):
        fixture = json.loads(Path(__file__).with_name('fixtures').joinpath('github-macos-crowded-tabs.json').read_text())
        ocr = desktop.settings.ocr
        with TemporaryDirectory() as temporary:
            image = Path(temporary) / 'capture.png'
            image.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\0' * 8 + struct.pack('>II', fixture['width'], fixture['height']))
            crowded = base64.b64decode(fixture['strips'][0]['pixelsBase64'])
            # The occupied strip remains invalid under the unchanged band guards.
            with patch.object(ocr.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, crowded)):
                with self.assertRaisesRegex(RuntimeError, 'bands do not match'):
                    ocr.workspace_geometry(image)
            def measure(command, **kwargs):
                filters = command[command.index('-vf')+1]
                for strip in fixture['strips']:
                    prefix = f"crop={strip['width']}:{fixture['height']//3}:{strip['left']}:0:exact=1,"
                    if filters.startswith(prefix):
                        return subprocess.CompletedProcess([], 0, base64.b64decode(strip['pixelsBase64']))
                self.fail(f'No captured pixels for measurement {filters}')
            driver = object.__new__(desktop.settings.Driver)
            driver.session = 'macos'
            with patch.object(ocr.sys, 'platform', 'darwin'), \
                    patch.object(ocr, 'recognize', return_value=fixture['words']) as recognize, \
                    patch.object(ocr.subprocess, 'run', side_effect=measure), \
                    patch.object(desktop.settings, 'record_measurement') as receipt:
                self.assertEqual(driver.locate(Mock(), image, 'sidebar', 'My PRs'),
                    dict(left=109, top=86, width=48, height=13))
                self.assertEqual(recognize.call_count, 2)
                self.assertEqual(receipt.call_count, 2)

    def test_sidebar_scope_rejects_missing_or_competing_controls_and_boundaries(self):
        words = json.loads(Path(__file__).with_name('fixtures').joinpath('github-pr-mode-sidebar.json').read_text())['words']
        target_words = [word for word in words if word['line'] == '9/1/1']
        # Keep the workspace title and page heading. Neither may stand in for
        # a missing sidebar control, even though the visible text is identical.
        without_target = [word for word in words if word not in target_words]
        with self.assertRaises(RuntimeError):
            desktop.settings.ocr.sidebar_label(without_target, 'My PRs')
        duplicates = [dict(word, line='duplicate-sidebar', top=word['top'] + 20) for word in target_words]
        with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
            desktop.settings.ocr.sidebar_label(words + duplicates, 'My PRs')
        with self.assertRaises(RuntimeError):
            desktop.settings.ocr.sidebar_label([word for word in words if word['text'] != 'Repositories'], 'My PRs')
        anchor = next(word for word in words if word['text'] == 'Repositories')
        with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
            desktop.settings.ocr.sidebar_label(words + [dict(anchor, line='duplicate-boundary', top=anchor['top']+35)], 'My PRs')

    def test_windows_driver_passes_the_explicit_sidebar_action_to_its_owned_input_helper(self):
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            driver = object.__new__(desktop.settings.Driver)
            driver.session, driver.profile = 'windows', root
            driver.config, driver.executable, driver.checkout = root/'config.json', root/'owned.exe', root
            driver.large_size, driver.backend = (1200, 820), Mock(url='http://127.0.0.1:9')
            with patch.object(desktop.settings, 'run') as execute:
                driver.drive('sidebar', [('sidebar', 'My PRs')])
            args = execute.call_args.args
            self.assertIn('sidebar64:' + desktop.settings.b64('My PRs'),
                args[args.index('-Actions')+1].split(','))

    def test_mac_sidebar_click_keeps_two_fresh_stable_measurements(self):
        words = json.loads(Path(__file__).with_name('fixtures').joinpath('github-pr-mode-sidebar.json').read_text())['words']
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        with TemporaryDirectory() as temporary, \
                patch.object(desktop.settings.ocr.sys, 'platform', 'darwin'), \
                patch.object(desktop.settings.ocr, 'recognize', return_value=words) as recognize, \
                patch.object(desktop.settings, 'record_measurement') as receipt:
            target = driver.locate(Mock(), Path(temporary)/'capture.png', 'sidebar', 'My PRs')
            self.assertEqual(target, dict(left=109, top=87, width=49, height=13))
            self.assertEqual(recognize.call_count, 2)
            self.assertEqual(receipt.call_count, 2)
            self.assertTrue(all(call.kwargs['scope']['sidebar'] for call in receipt.call_args_list))

    def select_refresh(self, words, action):
        return desktop.settings.ocr.locate_label(words, action[1],
            content=action[0] == 'content', between=action[2:] if action[0] == 'between' else None)

    def test_recorded_refresh_guard_alternates_between_static_prose_and_toolbar(self):
        fixture = self.refresh_fixture()
        targets = [desktop.settings.ocr.locate_label(fixture['wordLayouts'][frame['layout']], 'Refresh', content=True)
            for frame in fixture['layouts']['original']]
        self.assertEqual(targets, [frame['target'] for frame in fixture['layouts']['original']])
        self.assertTrue(all(not desktop.settings.stable(a, b) for a, b in zip(targets, targets[1:])))
        for diff in fixture['differences']:
            self.assertEqual(diff['changedPixels']['toolbarRefresh'], 0)
            self.assertEqual(diff['changedPixels']['acknowledgementRefresh'], 0)
            self.assertEqual(diff['otherPixels'], 0)

    def test_actual_review_flow_selects_the_toolbar_in_one_layout_per_capture(self):
        fixture, action = self.refresh_fixture(), self.review_refresh_action()
        selected = []
        for original, scoped in zip(fixture['layouts']['original'], fixture['layouts']['scoped']):
            for frame in (original, scoped):
                try:
                    target = self.select_refresh(fixture['wordLayouts'][frame['layout']], action)
                    break
                except desktop.settings.ocr.AmbiguousLabel:
                    raise
                except RuntimeError:
                    continue
            else:
                self.fail('No single captured layout identifies the toolbar Refresh')
            self.assertEqual(target, scoped['target'])
            selected.append(target)
        self.assertTrue(desktop.settings.stable(*selected))

    def test_acknowledgement_prose_cannot_substitute_for_a_missing_toolbar_button(self):
        fixture = self.refresh_fixture()
        words = fixture['wordLayouts'][fixture['layouts']['original'][0]['layout']]
        with self.assertRaises(RuntimeError):
            self.select_refresh(words, self.review_refresh_action())

    def test_refresh_requires_both_visible_toolbar_boundaries(self):
        fixture = self.refresh_fixture()
        words = fixture['wordLayouts'][fixture['layouts']['scoped'][0]['layout']]
        action = self.review_refresh_action()
        for missing in ('filter', 'conversation'):
            subset = [word for word in words if missing not in desktop.settings.ocr.normalize(word['text'])]
            with self.assertRaises(RuntimeError):
                self.select_refresh(subset, action)

    def review_fixture(self):
        return json.loads(Path(__file__).with_name('fixtures').joinpath('review-result-wrapped-words.json').read_text(encoding='utf-8'))

    def test_actual_flat_review_ocr_cannot_qualify_the_wrapped_notice(self):
        fixture = self.review_fixture()
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, 'run', return_value=fixture['rawTesseractText']):
            with self.assertRaisesRegex(RuntimeError, 'one OCR layout'):
                desktop.check(Path(temporary) / 'review.png', 'Captured review', 'Freshness has not been checked')

    def test_actual_review_word_positions_qualify_the_complete_detail_column(self):
        fixture = self.review_fixture()
        with TemporaryDirectory() as temporary, patch.object(desktop.settings.ocr, 'recognize', return_value=fixture['words']) as recognize:
            image = Path(temporary) / 'review.png'
            image.write_bytes(b'untouched original')
            desktop.check_column(image, 'Captured review', 'Captured review', 'Freshness has not been checked')
            self.assertEqual(recognize.call_count, 1)
            self.assertEqual(image.read_bytes(), b'untouched original')
            text = image.with_suffix('.column-ocr.txt').read_text(encoding='utf-8')
            self.assertIn('Freshness has not been checked', text)

    def test_other_column_cannot_supply_a_missing_notice_word(self):
        words = [word for word in self.review_fixture()['words']
                 if not (word['left'] >= 330 and word['text'] == 'not')]
        words.append(dict(text='not', left=80, top=485, width=18, height=9, line='neighbor'))
        with TemporaryDirectory() as temporary, patch.object(desktop.settings.ocr, 'recognize', return_value=words), \
                patch.object(desktop.settings, 'run', return_value=''):
            with self.assertRaisesRegex(RuntimeError, 'one OCR column layout'):
                desktop.check_column(Path(temporary) / 'review.png', 'Captured review', 'Captured review', 'Freshness has not been checked')

    def test_column_markers_from_separate_layouts_cannot_be_combined(self):
        words = self.review_fixture()['words']
        def recognize(_image, mode):
            omitted = {'not', 'been', 'checked'} if mode == 11 else {'Freshness', 'has'}
            return [word for word in words if not (word['left'] >= 330 and word['text'] in omitted)]
        with TemporaryDirectory() as temporary, patch.object(desktop.settings.ocr, 'recognize', side_effect=recognize), \
                patch.object(desktop.settings, 'run', return_value=''):
            with self.assertRaisesRegex(RuntimeError, 'one OCR column layout'):
                desktop.check_column(Path(temporary) / 'review.png', 'Captured review', 'Captured review', 'Freshness has not been checked')

    def test_competing_detail_column_anchor_is_not_retried_away(self):
        words = self.review_fixture()['words'] + [
            dict(text='Captured', left=340, top=600, width=53, height=13, line='other'),
            dict(text='review:', left=398, top=600, width=40, height=10, line='other'),
            dict(text='another', left=442, top=600, width=50, height=10, line='other')]
        with TemporaryDirectory() as temporary, patch.object(desktop.settings.ocr, 'recognize', return_value=words) as recognize:
            with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
                desktop.check_column(Path(temporary) / 'review.png', 'Captured review', 'Freshness has not been checked')
            self.assertEqual(recognize.call_count, 1)

    def test_column_engine_failure_remains_the_primary_failure(self):
        with TemporaryDirectory() as temporary, patch.object(desktop.settings.ocr, 'recognize', side_effect=RuntimeError('OCR engine failed')) as recognize:
            with self.assertRaisesRegex(RuntimeError, 'OCR engine failed'):
                desktop.check_column(Path(temporary) / 'review.png', 'Captured review', 'Freshness has not been checked')
            self.assertEqual(recognize.call_count, 1)

    def test_small_glyphs_use_an_analysis_copy_without_changing_the_original(self):
        calls = []

        def recognize(*arguments, **_kwargs):
            calls.append(arguments)
            if arguments[0] == "ffmpeg":
                return ""
            return ("src/navigation.rs native navigation Patch unavailable" if
                str(arguments[1]).endswith(".ocr-scale3.png") else "stc/navigation.s native navigation Patch unavailable")

        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=recognize):
            image = Path(temporary) / "diff.png"
            image.write_bytes(b"untouched original")
            desktop.check(image, "navigation.rs", "native navigation", "Patch unavailable")
            self.assertEqual(image.read_bytes(), b"untouched original")
            self.assertIn("src/navigation.rs", image.with_suffix(".ocr.txt").read_text(encoding="utf-8"))
        scalers = [call for call in calls if call[0] == "ffmpeg"]
        self.assertEqual(len(scalers), 1)
        self.assertIn("scale=iw*3:ih*3:flags=neighbor", scalers[0])
        self.assertEqual(len([call for call in calls if call[0] == "tesseract"]), 4)

    def test_partial_ocr_result_cannot_hide_an_account_leak(self):
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", return_value="Second private") as recognize:
            with self.assertRaisesRegex(RuntimeError, "Another account"):
                desktop.check(Path(temporary) / "github.png", "First private", absent=("Second private",))
            self.assertEqual(recognize.call_count, 1)

    def test_analysis_tool_errors_are_not_retried_or_replaced(self):
        def recognize(*arguments, **_kwargs):
            if arguments[0] == "ffmpeg":
                raise RuntimeError("Analysis tool failed")
            return "missing visible content"
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=recognize):
            with self.assertRaisesRegex(RuntimeError, "Analysis tool failed"):
                desktop.check(Path(temporary) / "github.png", "First private")

    def test_diff_markers_must_match_exactly_in_one_native_ocr_layout(self):
        outputs = {
            11: "stc/navigation.s native navigation Patch unavailable",
            3: "src/navigationrs native navigation Patch unavailable",
            6: "src/navigation.rs native navigation Patch unavailable",
        }
        modes = []

        def recognize(*arguments, **kwargs):
            mode = int(arguments[arguments.index("--psm") + 1])
            modes.append(mode)
            self.assertEqual(kwargs.get("encoding"), "utf-8")
            return outputs[mode]

        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=recognize):
            image = Path(temporary) / "diff.png"
            desktop.check(image, "navigation.rs", "native navigation", "Patch unavailable")
            self.assertIn("src/navigation.rs", image.with_suffix(".ocr.txt").read_text(encoding="utf-8"))
        self.assertEqual(modes, [11, 3, 6])

    def test_partial_markers_from_different_layouts_cannot_be_combined(self):
        outputs = {11: "navigation.rs", 3: "native navigation", 6: "Patch unavailable"}
        def recognize(*args, **_kwargs):
            return "" if args[0] == "ffmpeg" else outputs[int(args[args.index("--psm") + 1])]
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=recognize):
            with self.assertRaises(RuntimeError):
                desktop.check(Path(temporary) / "diff.png", "navigation.rs", "native navigation", "Patch unavailable")

    def test_cross_account_data_cannot_be_retried_away_by_another_ocr_layout(self):
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", return_value="First private Second private") as recognize:
            with self.assertRaisesRegex(RuntimeError, "Another account"):
                desktop.check(Path(temporary) / "github.png", "First private", absent=("Second private",))
            self.assertEqual(recognize.call_count, 1)

    def test_ocr_engine_failure_is_not_treated_as_missing_text(self):
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=RuntimeError("OCR engine failed")) as recognize:
            with self.assertRaisesRegex(RuntimeError, "OCR engine failed"):
                desktop.check(Path(temporary) / "github.png", "First private")
            self.assertEqual(recognize.call_count, 1)

    def test_utf8_ocr_receipts_are_read_independently_of_the_windows_code_page(self):
        read_text = Path.read_text

        def windows_default(path, *args, **kwargs):
            kwargs.setdefault("encoding", "cp1252")
            return read_text(path, *args, **kwargs)

        with TemporaryDirectory() as temporary:
            image = Path(temporary) / "github.png"
            with patch.object(desktop.settings, "run", return_value="First private List \u201d"), patch.object(
                Path, "read_text", autospec=True, side_effect=windows_default
            ):
                desktop.check(image, "First private", "List")
            self.assertEqual(image.with_suffix(".ocr.txt").read_text(encoding="utf-8"), "first private list \u201d")

    def test_failed_navigation_preserves_the_observed_requests(self):
        requests = self.detail_requests()
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            desktop.preserve_failed_requests(output, requests, RuntimeError("original navigation failure"))
            self.assertEqual(json.loads((output / "requests.json").read_text()), requests)

    def test_diagnostic_write_failure_does_not_replace_the_original_failure(self):
        output = Mock()
        output.__truediv__ = Mock(return_value=Mock(write_text=Mock(side_effect=OSError("unavailable"))))
        failure = RuntimeError("original navigation failure")
        desktop.preserve_failed_requests(output, self.detail_requests(), failure)
        self.assertEqual(str(failure), "original navigation failure")
        self.assertEqual(failure.__notes__, ["Request diagnostic failed: OSError"])

    def detail_requests(self):
        graphql = [dict(account="fixture-one", method="POST", path="/graphql", query="query " + operation)
            for operation in ("NativePullComments", "NativePullReviews", "NativePullCommits", "NativePullThreads", "NativeThreadComments")]
        rest = [dict(account="fixture-one", method="GET", path="/repos/fixture-org/native-demo" + path)
            for path in ("/pulls/7/files", "/commits/head/check-runs", "/commits/head/statuses", "/issues/7/timeline")]
        return graphql + rest

    def test_click_receipts_cannot_substitute_for_loading_the_review_tab(self):
        requests = self.detail_requests()
        desktop.verify_detail_requests(requests)
        requests = [request for request in requests if "NativePullReviews" not in request.get("query", "")]
        with self.assertRaisesRegex(RuntimeError, "NativePullReviews"):
            desktop.verify_detail_requests(requests)

    def test_another_accounts_detail_requests_do_not_qualify_the_selected_account(self):
        requests = [dict(request, account="fixture-two") for request in self.detail_requests()]
        with self.assertRaises(RuntimeError):
            desktop.verify_detail_requests(requests)

    def test_installed_version_must_match_exactly_before_accepting_its_hash(self):
        with TemporaryDirectory() as temporary:
            executable = Path(temporary) / "owned-binary"
            executable.write_bytes(b"synthetic executable")
            with patch.object(desktop.settings, "run", return_value="Buddy 0.1.13760"):
                with self.assertRaises(RuntimeError):
                    desktop.installed_binary(executable, "0.1.1376")
            with patch.object(desktop.settings, "run", return_value="Buddy 0.1.1376") as run:
                receipt = desktop.installed_binary(executable, "0.1.1376")
            run.assert_called_once_with(executable, "--version")
            self.assertEqual(receipt, dict(versionOutput="Buddy 0.1.1376",
                binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest()))

    def test_route_content_is_required_without_inventing_a_visible_version_label(self):
        with TemporaryDirectory() as temporary:
            image = Path(temporary) / "github.png"
            with patch.object(desktop.settings, "run", return_value="My PRs First private"):
                desktop.check(image, "My PRs", "First private")
            with patch.object(desktop.settings, "run", return_value="My PRs Loading"):
                with self.assertRaises(RuntimeError):
                    desktop.check(image, "My PRs", "First private")
            with patch.object(desktop.settings, "run", return_value="My PRs First private Second private"):
                with self.assertRaisesRegex(RuntimeError, "Another account"):
                    desktop.check(image, "My PRs", "Second private", absent=("First private",))


if __name__ == "__main__":
    unittest.main()
