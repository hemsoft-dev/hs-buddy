"""Headless contract tests. Every native API is mocked before use."""
import base64
import ast
from copy import deepcopy
from http.client import HTTPConnection, RemoteDisconnected
import importlib.util
import json
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import Mock, patch
from urllib.parse import urlsplit

spec = importlib.util.spec_from_file_location('tempo_desktop', Path(__file__).with_name('tempo-desktop.py'))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class FixtureScenarios(unittest.TestCase):
    def test_real_http_preflight_counts_only_mutations_after_recorded_reads(self):
        with desktop.ScenarioFixture() as api:
            api.preflight_after_lookup = True
            jira_auth = 'Basic ' + base64.b64encode((desktop.tempo.JIRA_EMAIL + ':' + desktop.tempo.JIRA_TOKEN).encode()).decode()
            tempo_auth = 'Bearer ' + desktop.tempo.TEMPO_TOKEN
            address = urlsplit(api.url)
            def request(method, path, authorization, body=None):
                connection = HTTPConnection(address.hostname, address.port, timeout=3)
                try:
                    connection.request(method, path, body=json.dumps(body) if body is not None else None,
                        headers={'Authorization': authorization})
                    response = connection.getresponse()
                    response.read()
                    return response.status
                finally:
                    connection.close()
            self.assertEqual(request('GET', '/4/accounts?limit=50&offset=0', tempo_auth), 200)
            for expected in [200, 200, 503, 200]:
                self.assertEqual(request('GET', '/rest/api/3/myself', jira_auth), expected)
            self.assertEqual(len(api.snapshot()['attempts']), 5)
            self.assertEqual(api.snapshot()['preflightFailures'], [dict(writes=0, mutationAttempts=0)])
            self.assertTrue(all(row['method'] == 'GET' for row in api.snapshot()['attempts']))
            body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                startDate='2026-10-14', timeSpentSeconds=3600, attributes=[])
            self.assertEqual(request('POST', '/4/worklogs', tempo_auth, body), 200)
            state = desktop.require_writes(api, 1)
            self.assertEqual(len(state['attempts']), 6)
            self.assertEqual(desktop.write_body(state['writes'][0]), body)

    def test_installed_scenario_requires_the_regional_connection_including_its_template(self):
        with desktop.ScenarioFixture() as api:
            self.assertEqual(api.marker()['tempo_origin'], 'https://api.eu.tempo.io')
            self.assertTrue(all(row['self'].startswith(api.tempo_origin + '/4/worklogs/') for row in api.records.values()))
            self.assertTrue(all(row['attributes']['self'].startswith(api.tempo_origin + '/4/worklogs/') for row in api.records.values()))
            self.assertEqual((api.attempts, api.writes), ([], []))

    def test_real_permission_denial_records_request_without_changing_any_worklog(self):
        with desktop.ScenarioFixture() as api:
            before = deepcopy(api.records)
            api.deny_next_create = True
            body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                startDate='2026-10-14', timeSpentSeconds=5401, attributes=[])
            address = urlsplit(api.url)
            connection = HTTPConnection(address.hostname, address.port, timeout=3)
            try:
                connection.request('POST', '/4/worklogs', body=json.dumps(body),
                    headers={'Authorization': 'Bearer ' + desktop.tempo.TEMPO_TOKEN})
                response = connection.getresponse()
                self.assertEqual(response.status, 403)
                self.assertEqual(json.loads(response.read()), {'error': 'Synthetic worklog permission denied'})
            finally:
                connection.close()
            state = desktop.require_writes(api, 0, rejected=1)
            self.assertEqual(desktop.write_body(state['rejections'][0]), body)
            self.assertEqual(state['records'], before)
            self.assertFalse(api.deny_next_create)
            with self.assertRaisesRegex(RuntimeError, 'unexpected number'):
                desktop.require_writes(api, 0)

    def test_owned_seed_has_complete_pager_today_stack_template_and_three_source_logs(self):
        api = desktop.ScenarioFixture()
        original = api.records[desktop.tempo.FIRST_ID]
        self.assertEqual(original['startDate'], '2026-10-15')
        today = [row for row in api.records.values() if row['startDate'] == '2026-10-16']
        self.assertEqual((len(today), sum(row['timeSpentSeconds'] for row in today)), (47, 2820))
        self.assertEqual(sum(row['startDate'].startswith('2026-10') for row in api.records.values()), 51)
        self.assertEqual(sum(row['startDate'] == '2026-10-13' for row in api.records.values()), 3)
        template, = [row for row in api.records.values() if row['startDate'].startswith('2026-09')]
        self.assertEqual(template['description'], 'Synthetic prior month template')
        self.assertFalse(any(row['key'] == '_Account_' for row in template['attributes']['values']))
        self.assertEqual((api.attempts, api.writes), ([], []))

    def test_real_lost_reply_records_exactly_two_posts_without_deduplication(self):
        with desktop.ScenarioFixture() as api:
            original = deepcopy(api.records[desktop.tempo.FIRST_ID])
            api.arm_lost_reply_after(1)
            body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                startDate='2026-10-16', timeSpentSeconds=60, attributes=[])
            address = urlsplit(api.url)
            for index in range(2):
                connection = HTTPConnection(address.hostname, address.port, timeout=3)
                try:
                    connection.request('POST', '/4/worklogs', body=json.dumps(body),
                        headers={'Authorization': 'Bearer ' + desktop.tempo.TEMPO_TOKEN})
                    if index == 0:
                        response = connection.getresponse()
                        self.assertEqual(response.status, 200)
                        response.read()
                    else:
                        with self.assertRaises(RemoteDisconnected):
                            connection.getresponse()
                finally:
                    connection.close()
            state = desktop.require_writes(api, 2)
            self.assertEqual(len(state['records']), 54)
            self.assertEqual(state['records'][desktop.tempo.FIRST_ID], original)
            self.assertIsNone(api.drop_after)
            with self.assertRaisesRegex(RuntimeError, 'unexpected number'):
                desktop.require_writes(api, 1)

    def test_public_entrypoint_refuses_home_before_parsing_or_launching_anything(self):
        with patch.dict(os.environ, {}, clear=True), \
                patch.object(desktop.github, 'installed_binary') as installed, \
                patch.object(desktop.settings, 'SettingsBackend') as backend:
            with self.assertRaisesRegex(RuntimeError, 'owned disposable'):
                desktop.main()
        installed.assert_not_called()
        backend.assert_not_called()

    def test_unverified_package_source_refuses_before_running_an_installed_binary(self):
        with patch.object(desktop.github, 'installed_binary') as installed:
            with self.assertRaisesRegex(RuntimeError, 'package source provenance'):
                desktop.qualify(Path('never-launch'), Path('never-write'), 'windows', '0.1.0', 'invalid', Mock())
        installed.assert_not_called()

    def test_expiry_arms_only_after_a_billing_lookup_and_rejects_one_identity_read(self):
        with desktop.ScenarioFixture() as api:
            api.expire_after_lookup = True
            with api.lock:
                first = api.respond('GET', '/rest/api/3/myself', 'Basic ' + base64.b64encode(
                    (desktop.tempo.JIRA_EMAIL + ':' + desktop.tempo.JIRA_TOKEN).encode()).decode(), b'')
                self.assertEqual(first[0], 200)
                self.assertFalse(api.reject_identity)
                catalog = api.respond('GET', '/4/accounts?limit=50&offset=0', 'Bearer ' + desktop.tempo.TEMPO_TOKEN, b'')
                self.assertEqual(catalog[0], 200)
                authorization = 'Basic ' + base64.b64encode((desktop.tempo.JIRA_EMAIL + ':' + desktop.tempo.JIRA_TOKEN).encode()).decode()
                self.assertEqual(api.respond('GET', '/rest/api/3/myself', authorization, b'')[0], 401)
                self.assertEqual(api.respond('GET', '/rest/api/3/myself', authorization, b'')[0], 200)
            self.assertEqual(api.writes, [])


class CellInput(unittest.TestCase):
    @staticmethod
    def stage_actions(stage, session):
        tree = ast.parse(Path(desktop.__file__).read_text(encoding='utf-8'))
        call, = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute) and node.func.attr == 'drive'
            and node.args and isinstance(node.args[0], ast.Constant) and node.args[0].value == stage]
        expression = ast.Expression(call.args[1])
        for node in ast.walk(expression):
            if isinstance(node, ast.Call) and not (isinstance(node.func, ast.Name) and node.func.id == 'week_actions'):
                raise AssertionError('Stage action construction must remain headless')
        return eval(compile(expression, str(desktop.__file__), 'eval'),
            dict(__builtins__={}, tempo=desktop.tempo, week_actions=desktop.week_actions, session=session))

    def test_timeline_pager_gets_a_coarse_owned_scroll_before_expensive_ocr_waits(self):
        actions = self.stage_actions('grid-navigation-dark', 'windows')
        cleared = actions.index(('click', 'Show all worklogs'))
        waiting = next(index for index, action in enumerate(actions) if action[:2] == ('wait_scroll_text', 'Next rows'))
        self.assertIn(('wheel', -7200), actions[cleared + 1:waiting])
        self.assertIn(('click', 'Next rows'), actions[waiting + 1:])
        self.assertIn(('wait_text', 'page 2'), actions[waiting + 1:])

    @staticmethod
    def driven(stage, session):
        """Actions the Tempo driver hands to the shared driver for one stage."""
        driver = object.__new__(desktop.TempoDriver)
        driver.session = session
        with patch.object(desktop.settings.Driver, 'drive', autospec=True) as drive:
            driver.drive(stage, CellInput.stage_actions(stage, session))
        return drive.call_args.args[2]

    def test_every_mac_tempo_editor_gets_one_bounded_offset_and_other_sessions_are_unchanged(self):
        stages = ('connect-dark', 'range-navigation-dark', 'grid-navigation-dark', 'template-dark', 'denied-write-dark')
        for stage in stages:
            mac = [action for action in self.driven(stage, 'macos') if action[0] == 'input']
            self.assertTrue(mac, stage)
            for action in mac:
                if action[1] in ('From', 'Through'):
                    # ARM run on e0a4da68: these editors start 17pt below their
                    # labels, so 30pt reached the Through border and typed nothing.
                    self.assertEqual(len(action), 3, action[:2])
                    continue
                self.assertEqual(sum(isinstance(item, dict) for item in action), 1, action[:2])
                self.assertEqual(action[-1], {'labelBottomOffset': 30}, action[:2])
            for session in ('windows', 'x11'):
                self.assertEqual(self.driven(stage, session), self.stage_actions(stage, session))
        start, = [action for action in self.driven('denied-write-dark', 'macos')
            if action[:2] == ('input', 'Start time in the account timezone')]
        self.assertEqual(start[3:], ({'labelBottomOffset': 30},))

    def test_mac_jira_password_uses_the_measured_inner_editor_without_changing_windows_spacing(self):
        actions = self.driven('connect-dark', 'macos')
        action, = [action for action in actions if action[:2] == ('input', 'Jira API token')]
        windows = self.driven('connect-dark', 'windows')
        self.assertEqual(len(next(action for action in windows if action[:2] == ('input', 'Jira API token'))), 3)
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        driver.locate = Mock(return_value=dict(left=64, top=552, width=80, height=10))
        driver.input_receipt = Mock()
        helper, window = Mock(), Mock(process='owned-pid')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = root / 'locator.png'
            image.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\x00\x00\x00\rIHDR' + struct.pack('>II', 1200, 820))
            image.with_suffix('.window.json').write_text(json.dumps(dict(
                capturePixelsPerWindowPoint=1, kCGWindowBounds=dict(X=80, Y=80, Width=1200, Height=820))))
            with patch.object(desktop.settings, 'OwnedMacInput', return_value=helper), \
                    patch.object(desktop.settings, 'run'), patch.object(desktop.settings.time, 'sleep'):
                driver.interact(window, root, action[0], list(action[1:]))
        helper.click.assert_called_once_with(164, 672)
        helper.replace_text.assert_called_once_with(desktop.tempo.JIRA_TOKEN)

    def test_mac_connection_inputs_all_target_the_inner_editor_independent_of_label_descenders(self):
        labels = ('Tempo API token', 'Jira HTTPS origin', 'Jira email', 'Jira API token')
        mac = {action[1]: action for action in self.driven('connect-dark', 'macos') if action[0] == 'input'}
        windows = {action[1]: action for action in self.driven('connect-dark', 'windows') if action[0] == 'input'}
        self.assertEqual(set(mac), set(labels))
        for label in labels:
            self.assertEqual(mac[label][3:], ({'labelBottomOffset': 30},))
            self.assertEqual(len(windows[label]), 3)
        # Intel OCR measured "Jira email" without descenders (10pt high). The
        # default 20pt offset reached y=513, the top border of the 508..538
        # editor, so the field stayed empty and Save remained disabled.
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        driver.locate = Mock(return_value=dict(left=64, top=483, width=55, height=10))
        driver.input_receipt = Mock()
        helper, window = Mock(), Mock(process='owned-pid')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            image = root / 'locator.png'
            image.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\x00\x00\x00\rIHDR' + struct.pack('>II', 1200, 820))
            image.with_suffix('.window.json').write_text(json.dumps(dict(
                capturePixelsPerWindowPoint=1, kCGWindowBounds=dict(X=80, Y=80, Width=1200, Height=820))))
            with patch.object(desktop.settings, 'OwnedMacInput', return_value=helper), \
                    patch.object(desktop.settings, 'run'), patch.object(desktop.settings.time, 'sleep'):
                action = mac['Jira email']
                driver.interact(window, root, action[0], list(action[1:]))
        helper.click.assert_called_once_with(164, 603)
        helper.replace_text.assert_called_once_with(desktop.tempo.JIRA_EMAIL)

    def test_retained_value_captures_move_focus_off_the_checked_editor(self):
        # Ubuntu run on 8adcc55a: Tesseract read every unfocused editor but
        # never the focused Hours value 1:30:01 inside its focus ring.
        denied = self.stage_actions('denied-write-dark', 'x11')
        last_hours = max(index for index, action in enumerate(denied) if action[:2] == ('input', 'Hours or H:MM:SS'))
        capture = denied.index(('capture', 'denied-hours-retained-dark'))
        self.assertIn(('key', 'Tab'), denied[last_hours + 1:capture])
        self.assertFalse([action for action in denied[last_hours + 1:capture] if action[0] == 'input'])
        # Description is a multi-line textarea, where Tab is not a focus move.
        # Type it first and leave focus from the single-line Hours editor.
        reauth = self.stage_actions('reauth-draft-dark', 'x11')
        inputs = [index for index, action in enumerate(reauth) if action[0] == 'input']
        labels = [reauth[index][1] for index in inputs]
        self.assertEqual(labels, ['Issue key or ID', 'Description', 'Hours or H:MM:SS'])
        self.assertEqual(reauth[inputs[-1] + 1], ('key', 'Tab'))
        self.assertNotEqual(reauth[inputs[1] + 1], ('key', 'Tab'))

    def test_description_editor_is_revealed_before_typing(self):
        # A visible label can leave its textarea below the viewport; the empty
        # editor's placeholder proves the input itself is on screen.
        for stage in ('denied-write-dark', 'reauth-draft-dark'):
            actions = self.stage_actions(stage, 'x11')
            typed = [index for index, action in enumerate(actions) if action[:2] == ('input', 'Description')]
            self.assertTrue(typed, stage)
            for index in typed:
                self.assertEqual(actions[index - 1], ('reveal_text', 'Work description', 'Description'), stage)

    def test_retained_draft_is_captured_between_its_description_and_billing_choice(self):
        # Run on 4b83e196: one coarse scroll from Hours put Cancel editor at the
        # top on Ubuntu (draft above the view) and skipped it on Apple Silicon.
        reauth = self.stage_actions('reauth-draft-dark', 'x11')
        capture = reauth.index(('capture', 'retained-draft-after-lookup-dark'))
        self.assertEqual(reauth[capture - 2:capture], [
            ('wait_scroll_text', 'Description', 'Hours or H:MM:SS'),
            ('reveal_text', 'No billing account', 'Description')])
        self.assertEqual(reauth[capture + 1:], [
            ('reveal_text', 'Cancel editor', 'No billing account'), ('click', 'Cancel editor')])

    def test_partial_copy_outcome_wait_covers_the_paced_request_sequence(self):
        # Windows run on 9a39095d: about 24 one-second-paced requests follow
        # Create copies, so the result was still "Working with Tempo..." when
        # the ordinary 15-second status wait ended.
        partial = self.stage_actions('partial-copy-dark', 'x11')
        click = partial.index(('click', 'Create copies'))
        self.assertEqual(partial[click + 1:click + 3], [('sleep', 30), ('wait_scroll_up', '1 worklog copied to Today')])

    def test_forget_status_is_found_above_the_scrolled_connection_panel(self):
        # Forget saved connection is reached by scrolling down; its status is
        # rendered at the top of the page, as with the recheck status.
        forget = self.stage_actions('forget-light', 'x11')
        click = forget.index(('click', 'Forget saved connection'))
        self.assertEqual(forget[click + 1], ('wait_scroll_up', 'credentials were removed'))

    def test_recheck_status_is_found_after_the_connection_panel_closes(self):
        # Ubuntu run on d92a456a: after Recheck the view sat in the Timeline
        # list, above which the retained-draft status was never searched.
        reauth = self.stage_actions('reauth-draft-dark', 'x11')
        recheck = reauth.index(('click', 'Recheck saved connection'))
        self.assertEqual(reauth[recheck + 1], ('wait_scroll_up', 'Your draft was retained'))

    def test_retained_value_captures_reveal_the_whole_editor_not_only_its_label(self):
        # Ubuntu run on e0a4da68: the Description label was visible, so the
        # wait never scrolled and the retained value was cut off below.
        denied = self.stage_actions('denied-write-dark', 'x11')
        capture = denied.index(('capture', 'denied-description-retained-dark'))
        self.assertEqual(denied[capture - 1], ('reveal_text', 'Denied native draft', 'Description'))
        reauth = self.stage_actions('reauth-draft-dark', 'x11')
        capture = reauth.index(('capture', 'retained-hours-after-lookup-dark'))
        self.assertEqual(reauth[capture - 1], ('reveal_text', 'Work date', 'Hours or H:MM:SS'))

    def test_template_draft_evidence_requires_only_text_visible_at_the_scrolled_editor_end(self):
        tree = ast.parse(Path(desktop.__file__).read_text(encoding='utf-8'))
        markers, = [[argument.value for argument in node.args[1:]] for node in ast.walk(tree)
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == 'check'
            and isinstance(node.args[0], ast.BinOp) and getattr(node.args[0].right, 'value', None) == 'template-new-draft-dark.png']
        # The capture follows a downward scroll to the billing choices, so the
        # editor heading is above the viewport. Create worklog, not Update
        # worklog, is the visible proof that this is a new draft.
        self.assertNotIn('Log time', markers)
        self.assertEqual(markers, ['Create worklog', 'No billing account', 'Synthetic prior month template'])

    def test_upward_wait_scrolls_the_owned_pane_up_and_requires_fresh_visible_text(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'x11'
        driver.scroll_anchor = Mock(return_value=(120, 500))
        driver.scroll = Mock()
        with patch.object(desktop.settings.ocr, 'locate_image', side_effect=[RuntimeError('offscreen'), dict(left=120)]), \
                patch.object(desktop.settings.time, 'monotonic', side_effect=[0, 1]), patch.object(desktop.settings.time, 'sleep'):
            window = Mock()
            driver.wait_scrolled(window, Path('mocked-capture'), 'Refresh', 'Description', up=True)
        driver.scroll.assert_called_once_with(window, 1200, (120, 500), precise=False)

    def test_windows_encoding_preserves_two_visible_labels_without_sending_native_input(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session, driver.large_size = 'windows', (1200, 820)
        driver.executable, driver.backend, driver.checkout = Path('installed'), Mock(url='loopback'), Path('repo')
        with tempfile.TemporaryDirectory() as directory:
            driver.profile, driver.config = Path(directory), Path(directory) / 'config.json'
            with patch.object(desktop.settings, 'run') as run:
                driver.drive('cell', [('cell', 'FIX-17', '14 Oct')])
            arguments = run.call_args.args
            command = arguments[arguments.index('-Actions') + 1]
            encoded = next(item for item in command.split(',') if item.startswith('cell64:'))
            self.assertEqual([base64.b64decode(item).decode() for item in encoded.split(':')[1:]], ['FIX-17', '14 Oct'])

    def test_retina_cell_uses_owned_origin_and_one_click_after_stable_measurements(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        driver.input_receipt = Mock()
        point = dict(x=840, y=696, column={}, row={})
        owned = Mock()
        window = Mock(process='owned-pid')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def screenshot(path):
                path.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\x00\x00\x00\rIHDR' + struct.pack('>II', 2400, 1640))
                path.with_suffix('.window.json').write_text(json.dumps(dict(
                    capturePixelsPerWindowPoint=2, kCGWindowBounds=dict(X=80, Y=40, Width=1200, Height=820))))
            window.screenshot.side_effect = screenshot
            with patch.object(desktop.settings, 'load', return_value=Mock(locate_image=Mock(return_value=point))), \
                    patch.object(desktop.settings, 'OwnedMacInput', return_value=owned), \
                    patch.object(desktop.settings, 'run'), patch.object(desktop.settings.time, 'sleep'):
                driver.interact(window, root, 'cell', ['FIX-17', '14 Oct'])
        self.assertEqual(window.screenshot.call_count, 2)
        owned.click.assert_called_once_with(500, 388)
        driver.input_receipt.assert_called_once()

    def test_clipped_capture_refuses_all_mac_input(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        window, owned = Mock(), Mock()
        with tempfile.TemporaryDirectory() as directory:
            def screenshot(path):
                path.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\x00\x00\x00\rIHDR' + struct.pack('>II', 2400, 1640))
                path.with_suffix('.window.json').write_text(json.dumps(dict(
                    capturePixelsPerWindowPoint=2, kCGWindowBounds=dict(X=80, Y=40, Width=900, Height=650))))
            window.screenshot.side_effect = screenshot
            with patch.object(desktop.settings, 'load', return_value=Mock(locate_image=Mock(return_value=dict(x=840, y=696)))), \
                    patch.object(desktop.settings, 'OwnedMacInput', return_value=owned), patch.object(desktop.settings, 'run') as run:
                with self.assertRaisesRegex(RuntimeError, 'does not match'):
                    driver.interact(window, Path(directory), 'cell', ['FIX-17', '14 Oct'])
            run.assert_not_called()
        owned.click.assert_not_called()

    def test_unstable_mac_cell_is_measured_four_times_without_replaying_input(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        window, owned = Mock(), Mock()
        grid = Mock(locate_image=Mock(side_effect=[dict(x=index, y=100) for index in range(4)]))
        with patch.object(desktop.settings, 'load', return_value=grid), \
                patch.object(desktop.settings, 'OwnedMacInput', return_value=owned), patch.object(desktop.settings, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'did not become stable'):
                driver.interact(window, Path('mocked-capture'), 'cell', ['FIX-17', '14 Oct'])
        self.assertEqual(window.screenshot.call_count, 4)
        run.assert_not_called()
        owned.click.assert_not_called()


if __name__ == '__main__':
    unittest.main()
