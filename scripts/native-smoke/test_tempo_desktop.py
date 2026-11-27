"""Headless contract tests. Every native API is mocked before use."""
import base64
import ast
from copy import deepcopy
from http.client import HTTPConnection, RemoteDisconnected
import importlib.util
import hashlib
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
    def test_view_clicks_are_content_scoped_before_reaching_each_platform_driver(self):
        for session in ('macos', 'x11', 'windows'):
            with self.subTest(session=session):
                driver = object.__new__(desktop.TempoDriver)
                driver.session = session
                actions = [('click', 'Timesheet'), ('click', 'Timeline'), ('click', 'Refresh'), ('wait_text', 'Tempo timesheet')]
                with patch.object(desktop.settings.Driver, 'drive', return_value='owned') as drive:
                    self.assertEqual(driver.drive('scene', actions), 'owned')
                self.assertEqual(drive.call_args.args[1], [('content', 'Timesheet'), ('content', 'Timeline'), ('content', 'Refresh'), ('wait_text', 'Tempo timesheet')])

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
            # The catalog read alone never arms the preflight fault.
            self.assertEqual(request('GET', '/4/accounts?limit=50&offset=0', tempo_auth), 200)
            self.assertTrue(api.preflight_after_lookup)
            self.assertEqual(request('GET', f'/4/account-links/project/{desktop.tempo.PROJECT_ID}?includeGlobalAccounts=true', tempo_auth), 200)
            for expected in [200, 200, 503, 200]:
                self.assertEqual(request('GET', '/rest/api/3/myself', jira_auth), expected)
            self.assertEqual(len(api.snapshot()['attempts']), 6)
            self.assertEqual(api.snapshot()['preflightFailures'], [dict(writes=0, mutationAttempts=0)])
            self.assertTrue(all(row['method'] == 'GET' for row in api.snapshot()['attempts']))
            body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                startDate='2026-10-14', timeSpentSeconds=3600, attributes=[])
            self.assertEqual(request('POST', '/4/worklogs', tempo_auth, body), 200)
            state = desktop.require_writes(api, 1)
            self.assertEqual(len(state['attempts']), 7)
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

    def test_real_right_click_delete_must_remove_exactly_the_single_cell_worklog(self):
        with desktop.ScenarioFixture() as api:
            tempo_auth = 'Bearer ' + desktop.tempo.TEMPO_TOKEN
            address = urlsplit(api.url)
            def request(method, path, body=None):
                connection = HTTPConnection(address.hostname, address.port, timeout=3)
                try:
                    connection.request(method, path, body=json.dumps(body) if body is not None else None,
                        headers={'Authorization': tempo_auth})
                    response = connection.getresponse()
                    response.read()
                    return response.status
                finally:
                    connection.close()
            # The seed's 14 October cell is empty and 13 October holds three.
            for date in ('2026-10-14', '2026-10-13'):
                with self.assertRaisesRegex(RuntimeError, 'exactly one worklog'):
                    desktop.single_cell_worklog(api, date)
            self.assertEqual(desktop.single_cell_worklog(api, '2026-10-15')[0], desktop.tempo.FIRST_ID)
            body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                startDate='2026-10-14', timeSpentSeconds=3600, attributes=[])
            api.deny_next_create = True
            self.assertEqual(request('POST', '/4/worklogs', body), 403)
            self.assertEqual(request('POST', '/4/worklogs', body), 200)
            target, removed = desktop.single_cell_worklog(api, '2026-10-14')
            self.assertEqual(removed['timeSpentSeconds'], 3600)
            before = api.snapshot()['records']
            with self.assertRaisesRegex(RuntimeError, 'unexpected number'):
                desktop.assert_cell_delete(api, before, target, 2)
            # The prompt alone sent nothing, so the fixture is unchanged.
            self.assertEqual(desktop.assert_cell_kept(api, before, 1)['records'], before)
            self.assertEqual(request('DELETE', f'/4/worklogs/{target}'), 204)
            # A DELETE sent by the right-click itself fails the prompt check.
            with self.assertRaisesRegex(RuntimeError, 'sent a write before its confirmation'):
                desktop.assert_cell_kept(api, before, 1)
            state = desktop.assert_cell_delete(api, before, target, 2)
            self.assertNotIn(target, state['records'])
            # Another worklog's deletion, or a stale "before", fails closed.
            with self.assertRaisesRegex(RuntimeError, 'exactly its single cell worklog'):
                desktop.assert_cell_delete(api, before, desktop.tempo.FIRST_ID, 2)
            with self.assertRaisesRegex(RuntimeError, 'exactly its single cell worklog'):
                desktop.assert_cell_delete(api, state['records'], target, 2)
            # A changed record without a counted write fails it too.
            with self.assertRaisesRegex(RuntimeError, 'changed worklogs before its confirmation'):
                desktop.assert_cell_kept(api, before, 2)

    def test_account_fallback_requires_a_catalog_read_without_a_lookup_or_write(self):
        with desktop.ScenarioFixture() as api:
            tempo_auth = 'Bearer ' + desktop.tempo.TEMPO_TOKEN
            address = urlsplit(api.url)
            def request(method, path, body=None):
                connection = HTTPConnection(address.hostname, address.port, timeout=3)
                try:
                    connection.request(method, path, body=json.dumps(body) if body is not None else None,
                        headers={'Authorization': tempo_auth})
                    response = connection.getresponse()
                    response.read()
                    return response.status
                finally:
                    connection.close()
            records = api.snapshot()['records']
            before = api.snapshot()['attempts']
            # Opening the draft alone read nothing: the catalog is required.
            with self.assertRaisesRegex(RuntimeError, 'did not read the Tempo account catalog'):
                desktop.assert_account_fallback(api, before, records)
            self.assertEqual(request('GET', '/4/accounts?limit=50&offset=0'), 200)
            state = desktop.assert_account_fallback(api, before, records)
            self.assertEqual(state['writes'], [])
            # A project lookup is not the every-account list.
            links = f'/4/account-links/project/{desktop.tempo.PROJECT_ID}?includeGlobalAccounts=true'
            self.assertEqual(request('GET', links), 200)
            with self.assertRaisesRegex(RuntimeError, 'project lookup'):
                desktop.assert_account_fallback(api, before, records)
            # Any write, or a changed worklog, fails the step too.
            after_lookup = api.snapshot()['attempts']
            self.assertEqual(request('GET', '/4/accounts?limit=50&offset=0'), 200)
            body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                startDate='2026-10-14', timeSpentSeconds=3600, attributes=[])
            self.assertEqual(request('POST', '/4/worklogs', body), 200)
            with self.assertRaisesRegex(RuntimeError, 'changed worklogs or sent a write'):
                desktop.assert_account_fallback(api, after_lookup, records)

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
                # A new draft's catalog read is not a lookup and arms nothing.
                catalog = api.respond('GET', '/4/accounts?limit=50&offset=0', 'Bearer ' + desktop.tempo.TEMPO_TOKEN, b'')
                self.assertEqual(catalog[0], 200)
                self.assertFalse(api.reject_identity)
                links = api.respond('GET', f'/4/account-links/project/{desktop.tempo.PROJECT_ID}?includeGlobalAccounts=true', 'Bearer ' + desktop.tempo.TEMPO_TOKEN, b'')
                self.assertEqual(links[0], 200)
                authorization = 'Basic ' + base64.b64encode((desktop.tempo.JIRA_EMAIL + ':' + desktop.tempo.JIRA_TOKEN).encode()).decode()
                self.assertEqual(api.respond('GET', '/rest/api/3/myself', authorization, b'')[0], 401)
                self.assertEqual(api.respond('GET', '/rest/api/3/myself', authorization, b'')[0], 200)
            self.assertEqual(api.writes, [])


class ReadyEscapeIntegration(unittest.TestCase):
    @staticmethod
    def request(api, method, path, body=None):
        address = urlsplit(api.url)
        authorization = ('Basic ' + base64.b64encode((desktop.tempo.JIRA_EMAIL + ':' + desktop.tempo.JIRA_TOKEN).encode()).decode()
            if path.startswith('/rest/') else 'Bearer ' + desktop.tempo.TEMPO_TOKEN)
        connection = HTTPConnection(address.hostname, address.port, timeout=3)
        try:
            connection.request(method, path, body=json.dumps(body) if body is not None else None,
                headers={'Authorization': authorization})
            response = connection.getresponse()
            response.read()
            return response.status
        finally:
            connection.close()

    def capture_ready(self, api, config):
        api.checkpoint_config = config
        api.preflight_after_lookup = True
        self.assertEqual(self.request(api, 'GET', f'/4/account-links/project/{desktop.tempo.PROJECT_ID}?includeGlobalAccounts=true'), 200)
        for status in (200, 200, 503):
            self.assertEqual(self.request(api, 'GET', '/rest/api/3/myself'), status)
        self.assertEqual(len(api.snapshot()['readyCheckpoints']), 1)

    def test_http_observer_binds_original_checkpoint_and_service_records_before_retry(self):
        with tempfile.TemporaryDirectory() as directory, desktop.ScenarioFixture() as api:
            config = Path(directory) / 'config.json'
            checkpoint = config.parent / 'native-tempo-change-synthetic.json'
            original = b'original synthetic checkpoint bytes\n'
            checkpoint.write_bytes(original)
            self.capture_ready(api, config)
            with self.assertRaisesRegex(RuntimeError, 'original Ready checkpoint'):
                desktop.require_ready_escape_checkpoints(api)  # An unfinished observation is not a pass.
            self.assertEqual(self.request(api, 'GET', '/rest/api/3/myself'), 200)
            proof = desktop.require_ready_escape_checkpoints(api)
            self.assertEqual(proof['checkpointSha256'], hashlib.sha256(original).hexdigest())
            self.assertEqual(checkpoint.read_bytes(), original)
            self.assertEqual([row['stage'] for row in proof['observations']],
                ['ready-before-escape', 'before-explicit-retry'])
            self.assertTrue(all(row['method'] == 'GET' for row in api.snapshot()['attempts']))
            self.assertEqual((len(api.snapshot()['attempts']), api.writes), (5, []))

    def test_changed_missing_duplicate_checkpoint_and_intervening_write_fail_closed(self):
        for change in ('changed', 'missing', 'duplicate', 'write'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory, desktop.ScenarioFixture() as api:
                config = Path(directory) / 'config.json'
                checkpoint = config.parent / 'native-tempo-change-synthetic.json'
                checkpoint.write_bytes(b'original synthetic recovery')
                self.capture_ready(api, config)
                if change == 'changed':
                    checkpoint.write_bytes(b'changed synthetic recovery')
                elif change == 'missing':
                    checkpoint.unlink()
                elif change == 'duplicate':
                    checkpoint.with_name('native-tempo-change-duplicate.json').write_bytes(b'duplicate')
                else:
                    body = dict(authorAccountId=desktop.tempo.ACCOUNT, issueId=desktop.tempo.ISSUE_ID,
                        startDate='2026-10-14', timeSpentSeconds=3600, attributes=[])
                    self.assertEqual(self.request(api, 'POST', '/4/worklogs', body), 200)
                self.assertEqual(self.request(api, 'GET', '/rest/api/3/myself'), 200)
                with self.assertRaisesRegex(RuntimeError, 'original Ready checkpoint'):
                    desktop.require_ready_escape_checkpoints(api)

    def test_ready_escape_capture_and_recording_stay_before_retry_in_one_native_process(self):
        tree = ast.parse(Path(desktop.__file__).read_text(encoding='utf-8'))
        call, = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute) and node.func.attr == 'drive'
            and node.args and isinstance(node.args[0], ast.Constant) and node.args[0].value == 'template-dark']
        self.assertTrue(ast.literal_eval(next(arg.value for arg in call.keywords if arg.arg == 'record')))
        actions = CellInput.stage_actions('template-dark', 'windows')
        before = actions.index(('capture', 'unsent-captured-change-dark'))
        escape = actions.index(('key', 'Escape'))
        captured = actions.index(('capture', 'ready-editor-after-escape-dark'))
        retry = actions.index(('click', 'Retry captured change'))
        self.assertLess(before, escape)
        self.assertLess(escape, captured)
        self.assertLess(captured, retry)
        self.assertIn(('wait_scroll_text', 'Hours or H:MM:SS', 'Tempo timesheet'), actions[escape:captured])
        self.assertFalse(any(action[0] == 'input' for action in actions[before:retry]))
        self.assertIn('readyEscapeRetainsEditorAndCheckpoint', Path(desktop.__file__).read_text())

    def test_idle_escape_scenes_capture_the_open_draft_then_its_absence_in_both_themes(self):
        source = Path(desktop.__file__).read_text(encoding='utf-8')
        tree = ast.parse(source)
        for stage, opened, cancelled, control, recorded in (
                ('escape-cancel-dark', 'escape-draft-open-dark', 'escape-draft-cancelled-dark', 'Hours or H:MM:SS', True),
                ('escape-cancel-light', 'escape-draft-open-compact-light', 'escape-draft-cancelled-compact-light',
                    'Issue key or ID', False)):
            with self.subTest(stage=stage):
                actions = CellInput.stage_actions(stage, 'x11')
                typed = next(index for index, action in enumerate(actions) if action[:2] == ('input', control))
                before = actions.index(('capture', opened))
                escape = actions.index(('key', 'Escape'))
                after = actions.index(('capture', cancelled))
                self.assertLess(typed, before)
                self.assertEqual(escape, before + 1)
                self.assertLess(escape, after)
                # Escape cancels from a focused editor control (#963): nothing
                # may move focus between typing into it and the key, and the
                # Cancel editor button must not be what closes the draft.
                self.assertFalse(any(action[0] in ('click', 'input', 'cell', 'key')
                    for action in actions[typed + 1:escape]))
                self.assertFalse(any(action[0] in ('click', 'input', 'cell') for action in actions[escape:after]))
                self.assertNotIn(('click', 'Cancel editor'), actions)
                call, = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Attribute) and node.func.attr == 'drive'
                    and node.args and isinstance(node.args[0], ast.Constant) and node.args[0].value == stage]
                self.assertEqual(any(arg.arg == 'record' and ast.literal_eval(arg.value) for arg in call.keywords), recorded)
        light = CellInput.stage_actions('escape-cancel-light', 'x11')
        self.assertLess(light.index(('resize', 900, 650)), light.index(('capture', 'escape-draft-open-compact-light')))
        self.assertFalse(any(action[0] == 'resize' for action in CellInput.stage_actions('escape-cancel-dark', 'x11')))
        # Each cancelled capture must prove the editor is gone, not merely that
        # the page still renders, and the open capture must show those labels.
        self.assertEqual(source.count('absent=IDLE_ESCAPE_EDITOR_LABELS'), 2)
        self.assertEqual(desktop.IDLE_ESCAPE_EDITOR_LABELS, ('Issue key or ID', 'Hours or H:MM:SS'))
        self.assertIn("idleEscapeRecording=recording_inventory(output, 'escape-cancel-dark')", source)

    def test_an_open_editor_fails_the_cancelled_capture_check(self):
        def verify(image, *markers, mode=11):
            image.with_suffix('.ocr.txt').write_text('tempo timesheet\nlog time\nissue key or id\nhours or h:mm:ss', encoding='utf-8')

        with tempfile.TemporaryDirectory() as directory:
            capture = Path(directory) / 'escape-draft-cancelled-dark.png'
            capture.write_bytes(b'capture')
            with patch.object(desktop.github, 'analysis_images', return_value=[capture]), \
                    patch.object(desktop.github.settings, 'verify_text', side_effect=verify):
                with self.assertRaises(RuntimeError):
                    desktop.github.check(capture, 'Tempo timesheet', absent=desktop.IDLE_ESCAPE_EDITOR_LABELS)

    def test_ready_recording_requires_original_video_and_frames(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with self.assertRaisesRegex(RuntimeError, 'recording'):
                desktop.recording_inventory(output, 'template-dark')
            scene = output / 'template-dark'
            (scene / 'frames').mkdir(parents=True)
            (scene / 'interaction.mp4').write_bytes(b'original encoded video fixture')
            probe = dict(streams=[dict(codec_type='video')], format=dict(duration='2.5'))
            with patch.object(desktop.settings, 'run', return_value=json.dumps(probe)):
                with self.assertRaisesRegex(RuntimeError, 'original unresized'):
                    desktop.recording_inventory(output, 'template-dark')
                for index in range(10):
                    (scene / 'frames' / f'frame-{index:03}.png').write_bytes(b'original frame fixture')
                receipt = desktop.recording_inventory(output, 'template-dark')
            self.assertEqual(receipt['path'], 'template-dark/interaction.mp4')
            self.assertEqual(receipt['originalFrameCount'], 10)
            self.assertEqual(receipt['sha256'], hashlib.sha256(b'original encoded video fixture').hexdigest())


class CellInput(unittest.TestCase):
    def test_real_windows_today_capture_requires_the_toolbar_scope_on_every_platform(self):
        fixture = json.loads(Path(desktop.__file__).with_name('fixtures').joinpath('tempo-windows-today-scope.json').read_text())
        ocr = desktop.settings.ocr
        for name, words in fixture['layouts'].items():
            with self.subTest(layout=name):
                target = ocr.find_label(words, fixture['target'], between=fixture['anchors'])
                self.assertEqual(target, dict(left=210, top=140, width=38, height=13))
                # Competing toolbar controls must still fail, even though
                # Today in warning/help prose below the totals is excluded.
                duplicate = words + [dict(target, text='Today', line='duplicate', left=400)]
                with self.assertRaises(ocr.AmbiguousLabel):
                    ocr.find_label(duplicate, fixture['target'], between=fixture['anchors'])
                with self.assertRaises(RuntimeError):
                    ocr.find_label(words, fixture['target'], between=('Missing heading', 'Logged'))
        # The Windows path first misses the menu in sparse Tesseract, then
        # correctly rejects four Today matches in block mode. Replay that
        # exact engine order rather than assuming every engine sees a collision.
        with tempfile.TemporaryDirectory() as directory:
            image = Path(directory) / 'owned.png'
            image.write_bytes(b'owned capture')
            sparse = fixture['layouts']['settings-locator.ocr-11.json']
            with patch.object(ocr.shutil, 'which', return_value='tesseract'), \
                    patch.object(ocr, 'recognize', side_effect=[sparse, fixture['ambiguousWords']]):
                with self.assertRaisesRegex(ocr.AmbiguousLabel, '4 visible matches'):
                    ocr.locate_image(image, 'Today', content=True)
            with patch.object(ocr.shutil, 'which', return_value='tesseract'), \
                    patch.object(ocr, 'recognize', return_value=sparse) as recognize:
                self.assertEqual(ocr.locate_image(image, 'Today', between=fixture['anchors']),
                    dict(left=210, top=140, width=38, height=13))
                recognize.assert_called_once_with(image, 11)
        for session in ('windows', 'macos', 'x11'):
            actions = self.driven('range-navigation-dark', session)
            today, = [action for action in actions if len(action) > 1 and action[1] == 'Today']
            self.assertEqual(today, ('between', 'Today', *fixture['anchors']))

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

    def test_the_prompt_drive_stops_at_the_confirmation_and_another_confirms_it(self):
        for session in ('windows', 'macos', 'x11'):
            with self.subTest(session=session):
                prompt = self.stage_actions('cell-delete-prompt-dark', session)
                confirm = self.stage_actions('cell-delete-dark', session)
                for actions in (prompt, confirm):
                    self.assertEqual(actions[:len(desktop.week_actions())], desktop.week_actions())
                    menu = actions.index(('cell_menu', 'FIX-17', '14 Oct'))
                    # Pinned above the scrolled page, so no scroll finds it.
                    self.assertEqual(actions[menu + 1], ('wait_text', 'Confirm deletion'))
                    # Only the secondary button reaches the cell, so no left click
                    # can open its editor or list instead.
                    self.assertFalse(any(action[0] == 'cell' for action in actions))
                    self.assertIn(('cell_menu', 'FIX-17', '14 Oct'), self.driven(
                        'cell-delete-prompt-dark' if actions is prompt else 'cell-delete-dark', session))
                marks = prompt.index(('capture', 'tempo-holiday-capex-dark'))
                menu = prompt.index(('cell_menu', 'FIX-17', '14 Oct'))
                self.assertEqual(prompt[marks - 1], ('wait_scroll_text', 'FIX-17', 'Load date range'))
                self.assertLess(marks, menu)
                # The prompt drive ends at the open confirmation, unanswered.
                self.assertEqual(prompt[-1], ('capture', 'tempo-cell-delete-dark'))
                self.assertEqual(len(prompt), menu + 3)
                self.assertNotIn(('click', 'Confirm deletion'), prompt)
                menu = confirm.index(('cell_menu', 'FIX-17', '14 Oct'))
                self.assertEqual(confirm[menu + 2:], [('click', 'Confirm deletion'),
                    ('wait_scroll_up', 'Tempo confirmed the worklog change')])

    def test_the_cell_delete_scene_sits_between_reauthentication_and_the_lost_reply(self):
        source = Path(desktop.__file__).read_text(encoding='utf-8')
        order = [source.index(f"driver.drive('{stage}'") for stage in
            ('template-dark', 'reauth-draft-dark', 'cell-delete-prompt-dark', 'cell-delete-dark', 'partial-copy-dark')]
        self.assertEqual(order, sorted(order))
        scene = source[order[2]:order[4]]
        # The fixture is checked between the prompt and its confirmation:
        # unchanged after the right-click, one DELETE after the confirmation.
        between = source[order[2]:order[3]]
        self.assertIn('assert_cell_kept(api, before_cell_delete, 3)', between)
        self.assertNotIn('assert_cell_delete', between)
        self.assertIn('assert_cell_delete(api, before_cell_delete, target, 4)', source[order[3]:order[4]])
        # The deleted worklog is put back even when the scene fails.
        self.assertIn('finally:', scene)
        self.assertIn('api.records.setdefault(target, removed)', scene)
        for capture in ('tempo-holiday-capex-dark', 'tempo-cell-delete-dark'):
            self.assertIn(f"'cell-delete-prompt-dark/{capture}.png'", source)

    def test_the_account_fallback_capture_waits_for_the_unlinked_account_before_any_lookup(self):
        source = Path(desktop.__file__).read_text(encoding='utf-8')
        for session in ('windows', 'macos', 'x11'):
            with self.subTest(session=session):
                actions = self.stage_actions('account-fallback-dark', session)
                opened = actions.index(('click', 'Log time'))
                unlinked = actions.index(('wait_scroll_text', desktop.tempo.UNLINKED_ACCOUNT_KEY, 'Description'))
                # The list arrives after the draft opens, so its own row is
                # awaited (it settles the layout) before the capture.
                self.assertLess(opened, unlinked)
                self.assertEqual(actions[unlinked + 1], ('capture', 'tempo-account-fallback-dark'))
                self.assertEqual(actions[-1], ('capture', 'tempo-account-fallback-dark'))
                # No lookup, input or choice: the project stays unknown.
                self.assertNotIn(('click', 'Look up issue and billing accounts'), actions)
                self.assertFalse(any(action[0] in ('input', 'cell', 'key') for action in actions))
                self.assertEqual([action for action in actions if action[0] == 'click'], [('click', 'Log time')])
        self.assertNotIn(desktop.tempo.UNLINKED_ACCOUNT_ID, (17, 18, 19))
        self.assertIn("'account-fallback-dark/tempo-account-fallback-dark.png'", source)
        self.assertIn("github.check(fallback / 'tempo-account-fallback-dark.png', tempo.UNLINKED_ACCOUNT_KEY,", source)
        self.assertIn('assert_account_fallback(api, reads_before, before_denied)', source)
        order = [source.index(f"driver.drive('{stage}'") for stage in
            ('denied-write-dark', 'account-fallback-dark', 'escape-cancel-dark')]
        self.assertEqual(order, sorted(order))

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

    def test_every_mac_tempo_editor_gets_one_bounded_offset_and_other_sessions_preserve_inputs(self):
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
                expected = [('content', *action[1:]) if action[0] == 'click' else action
                    for action in self.stage_actions(stage, session)]
                self.assertEqual(self.driven(stage, session), expected)
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
        # Run on 8238ef96: a Mac fine reveal moved only 42-84pt in 15 seconds.
        # One explicit wheel event (about 200pt on Mac) precedes each reveal.
        reauth = self.stage_actions('reauth-draft-dark', 'x11')
        capture = reauth.index(('capture', 'retained-draft-after-lookup-dark'))
        self.assertEqual(reauth[capture - 3:capture], [
            ('wait_scroll_text', 'Description', 'Hours or H:MM:SS'), ('wheel', -240),
            ('reveal_text', 'No billing account', 'Description')])
        self.assertEqual(reauth[capture + 1:], [
            ('wheel', -240), ('reveal_text', 'Cancel editor', 'No billing account'), ('click', 'Cancel editor')])

    def test_partial_copy_outcome_wait_covers_the_paced_request_sequence(self):
        # Windows run on 9a39095d: about 24 one-second-paced requests follow
        # Create copies, so the result was still "Working with Tempo..." when
        # the ordinary 15-second status wait ended.
        partial = self.stage_actions('partial-copy-dark', 'x11')
        click = partial.index(('click', 'Create copies'))
        self.assertEqual(partial[click + 1:click + 3], [('sleep', 30), ('wait_scroll_up', '1 worklog copied to Today')])

    def test_copy_confirmation_wait_covers_the_paced_source_reads(self):
        # Apple Silicon run on c14ca7c7: about 12 one-second-paced reads
        # follow Copy day, and one slow OCR pass used the rest of the
        # 15-second wait before the confirmation appeared.
        for stage, day in (('copy-dark', '15 Oct'), ('partial-copy-dark', '13 Oct')):
            actions = self.stage_actions(stage, 'x11')
            click = actions.index(('cell', 'Copy day to Today', day))
            self.assertEqual(actions[click + 1:click + 3], [('sleep', 10), ('wait_scroll_up', 'Copy worklogs to Today')])

    def test_forget_status_is_found_above_the_scrolled_connection_panel(self):
        # Forget saved connection is reached by scrolling down; its status is
        # rendered at the top of the page, as with the recheck status.
        forget = self.stage_actions('forget-light', 'x11')
        click = forget.index(('click', 'Forget saved connection'))
        self.assertEqual(forget[click + 1], ('wait_scroll_up', 'credentials were removed'))

    def test_confirmed_copy_outcome_wait_covers_the_range_restore(self):
        # Apple Silicon run on 9013d767: returning to the user's range adds
        # paced reads, and the outcome arrived after the 15-second wait.
        copied = self.stage_actions('copy-dark', 'x11')
        click = copied.index(('click', 'Create copies'))
        self.assertEqual(copied[click + 1:click + 3], [('sleep', 15), ('wait_scroll_up', '1 worklog copied to Today')])

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
        # The miss, the settle read before the first step, then the hit.
        with patch.object(desktop.settings.ocr, 'locate_image',
                    side_effect=[RuntimeError('offscreen'), RuntimeError('still offscreen'), dict(left=120)]) as locate, \
                patch.object(desktop.settings.time, 'monotonic', side_effect=[0, 0, 1]), patch.object(desktop.settings.time, 'sleep'):
            window = Mock()
            driver.wait_scrolled(window, Path('mocked-capture'), 'Refresh', 'Description', up=True)
        driver.scroll.assert_called_once_with(window, 1200, (120, 500), precise=False)
        self.assertEqual([call.kwargs for call in locate.call_args_list], [{}, dict(sparse_only=True), {}])

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

    def test_windows_cell_menu_is_its_own_command_with_the_same_two_labels(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session, driver.large_size = 'windows', (1200, 820)
        driver.executable, driver.backend, driver.checkout = Path('installed'), Mock(url='loopback'), Path('repo')
        with tempfile.TemporaryDirectory() as directory:
            driver.profile, driver.config = Path(directory), Path(directory) / 'config.json'
            with patch.object(desktop.settings, 'run') as run:
                driver.drive('cell', [('cell', 'FIX-17', '15 Oct'), ('cell_menu', 'FIX-17', '14 Oct')])
            arguments = run.call_args.args
            commands = arguments[arguments.index('-Actions') + 1].split(',')
        left, = [item for item in commands if item.startswith('cell64:')]
        menu, = [item for item in commands if item.startswith('cell_menu64:')]
        self.assertLess(commands.index(left), commands.index(menu))
        self.assertEqual([base64.b64decode(item).decode() for item in menu.split(':')[1:]], ['FIX-17', '14 Oct'])
        script = Path(desktop.__file__).with_name('windows-actions.ps1').read_text(encoding='utf-8')
        self.assertIn("'cell_menu64' {", script)

    def test_x11_cell_menu_presses_button_three_on_the_located_cell(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'x11'
        window = Mock(window='owned-window')
        grid = Mock(locate_image=Mock(return_value=dict(x=840.4, y=696.6)))
        for kind, button in (('cell', 1), ('cell_menu', 3)):
            with self.subTest(kind=kind), patch.object(desktop.settings, 'load', return_value=grid), \
                    patch.object(desktop.settings, 'run') as run, patch.object(desktop.settings.time, 'sleep'):
                driver.interact(window, Path('mocked-capture'), kind, ['FIX-17', '14 Oct'])
                self.assertEqual(run.call_args.args,
                    ('xdotool', 'mousemove', '--window', 'owned-window', 840, 696, 'click', button))

    def test_mac_cell_menu_posts_only_the_secondary_click(self):
        driver = object.__new__(desktop.settings.Driver)
        driver.session = 'macos'
        driver.input_receipt = Mock()
        point = dict(x=840, y=696, column={}, row={})
        owned = Mock()
        window = Mock(process='owned-pid')
        with tempfile.TemporaryDirectory() as directory:
            def screenshot(path):
                path.write_bytes(b'\x89PNG\r\n\x1a\n' + b'\x00\x00\x00\rIHDR' + struct.pack('>II', 2400, 1640))
                path.with_suffix('.window.json').write_text(json.dumps(dict(
                    capturePixelsPerWindowPoint=2, kCGWindowBounds=dict(X=80, Y=40, Width=1200, Height=820))))
            window.screenshot.side_effect = screenshot
            with patch.object(desktop.settings, 'load', return_value=Mock(locate_image=Mock(return_value=point))), \
                    patch.object(desktop.settings, 'OwnedMacInput', return_value=owned), \
                    patch.object(desktop.settings, 'run'), patch.object(desktop.settings.time, 'sleep'):
                driver.interact(window, Path(directory), 'cell_menu', ['FIX-17', '14 Oct'])
        owned.secondary_click.assert_called_once_with(500, 388)
        owned.click.assert_not_called()
        driver.input_receipt.assert_called_once()

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
        grid = Mock(locate_image=Mock(side_effect=[dict(x=index * 20, y=100) for index in range(4)]))
        with patch.object(desktop.settings, 'load', return_value=grid), \
                patch.object(desktop.settings, 'OwnedMacInput', return_value=owned), patch.object(desktop.settings, 'run') as run:
            with self.assertRaisesRegex(RuntimeError, 'did not become stable'):
                driver.interact(window, Path('mocked-capture'), 'cell', ['FIX-17', '14 Oct'])
        self.assertEqual(window.screenshot.call_count, 4)
        run.assert_not_called()
        owned.click.assert_not_called()


def blend(color, page, alpha):
    return tuple(round(alpha * c + (1 - alpha) * p) for c, p in zip(color, page))


def png(width, height, pixels):
    """A minimal RGB PNG, so ffmpeg decodes what the scene captures."""
    import zlib
    rows = b''.join(b'\x00' + pixels[y * width * 3:(y + 1) * width * 3] for y in range(height))
    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
        + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


class SyntheticGrid:
    """A drawn holiday/CapEx grid at a capture scale, with OCR anchor boxes."""
    PAGE = dict(dark=(0x1e, 0x1e, 0x1e), light=(0xff, 0xff, 0xff))
    TEXT = dict(dark=(0xcc, 0xcc, 0xcc), light=(0x1f, 0x1f, 0x1f))
    # The cell buttons' opaque secondary fill.
    BUTTON = dict(dark=(0x25, 0x25, 0x26), light=(0xf3, 0xf3, 0xf3))

    def __init__(self, theme='dark', scale=1, tint=True, marker=True, pill='success', cell='success', fringe=False,
            body_tint=None):
        tokens = desktop.THEME_MARKS[theme]
        self.theme, self.fringe = theme, fringe
        self.scale = s = scale
        self.width, self.height = 600 * s, 260 * s
        page, text = self.PAGE[theme], self.TEXT[theme]
        self.pixels = bytearray(bytes(page) * (self.width * self.height))
        plain, holiday = 200 * s, 292 * s
        # As render.rs lays it out: a 40-high issue row centred on the pill
        # at 165, then the 28-high Day total and Required rows to 241.
        tinted = blend(tokens['danger'], page, 0.08)
        if tint:
            self.fill(holiday, 100 * s, holiday + 92 * s, 145 * s, tinted)
        if tint if body_tint is None else body_tint:
            self.fill(holiday, 145 * s, holiday + 92 * s, 241 * s, tinted)
        for left in (plain, holiday):
            # The issue row's opaque Log and duration buttons cover its tint.
            self.fill(left + 2 * s, 149 * s, left + 90 * s, 181 * s, self.BUTTON[theme])
            for top in (192, 220):
                self.glyphs(dict(left=left + 4 * s, top=top * s, width=24 * s, height=8 * s), text)
        self.anchors = {}
        for left, name, weekday in ((plain, '14 Oct', True), (holiday, '15 Oct', not marker)):
            label = dict(left=left + 2 * s, top=125 * s, width=40 * s, height=10 * s)
            self.anchors[name] = label
            self.glyphs(label, text)
            line = dict(left=label['left'], top=108 * s, width=26 * s, height=10 * s)
            if weekday:
                self.glyphs(line, text)
            elif marker is not False:
                self.glyphs(line, tokens['danger'])
        pill_box = dict(left=40 * s, top=160 * s, width=50 * s, height=10 * s)
        self.anchors['FIX-17'] = pill_box
        pill_color = tokens[pill] if pill in tokens else text
        self.fill(pill_box['left'] - 7 * s, pill_box['top'] - 3 * s, pill_box['left'] + pill_box['width'] + 7 * s,
            pill_box['top'] + pill_box['height'] + 3 * s, blend(pill_color, page, 0.1))
        self.glyphs(pill_box, pill_color)
        cell_color = tokens[cell] if cell in tokens else text
        self.glyphs(dict(left=plain + 34 * s, top=160 * s, width=24 * s, height=10 * s), cell_color)

    def fill(self, left, top, right, bottom, color):
        for y in range(top, bottom):
            self.pixels[(y * self.width + left) * 3:(y * self.width + right) * 3] = bytes(color) * (right - left)

    def glyphs(self, box, color):
        # Vertical strokes two pixels wide every five, like text.
        tokens, page = desktop.THEME_MARKS[self.theme], self.PAGE[self.theme]
        family = next((name for name, token in tokens.items() if token == tuple(color)), None)
        top, bottom = box['top'], box['top'] + box['height']
        for x in range(box['left'], box['left'] + box['width'], 5 * self.scale):
            right = x + 2 * self.scale
            self.fill(x, top, right, bottom, color)
            # Subpixel edges, as X11 and ClearType captures show them: grey
            # text gets a danger-hued and an info-hued edge, green text an
            # info-hued edge on every other row. The danger marker has none.
            if self.fringe and family is None:
                self.fill(x - 1, top, x, bottom, blend(tokens['danger'], page, 0.7))
                self.fill(right, top, right + 1, bottom, blend(tokens['info'], page, 0.7))
            elif self.fringe and family == 'success':
                for y in range(top, bottom, 2):
                    self.fill(right, y, right + 1, y + 1, blend(tokens['info'], page, 0.7))

    def frame(self):
        return desktop.Frame(self.width, self.height, bytes(self.pixels))

    def words(self, scale=1):
        found = []
        for line, (name, box) in enumerate(self.anchors.items()):
            parts = name.split()
            step = box['width'] // len(parts)
            for index, part in enumerate(parts):
                found.append(dict(text=part, left=(box['left'] + index * step) // scale, top=box['top'] // scale,
                    width=(step - 2) // scale, height=box['height'] // scale, line=f'1/1/{line}'))
        return found


class GridMarks(unittest.TestCase):
    def test_holiday_and_capex_marks_pass_in_both_themes_at_each_capture_scale(self):
        for theme in ('dark', 'light'):
            for scale in (1, 2):
                with self.subTest(theme=theme, scale=scale):
                    grid = SyntheticGrid(theme, scale)
                    result = desktop.grid_marks(grid.frame(), grid.anchors, theme)
                    self.assertTrue(result['passed'])
                    self.assertGreaterEqual(result['holidayRednessShift'], 8)
                    self.assertGreaterEqual(result['holidayBodyRednessShift'], 8)
                    # The body is read from its fill, not the opaque buttons.
                    fill = result['bodyTints']['plain']
                    self.assertLessEqual(max(abs(a - b) for a, b in zip(fill, SyntheticGrid.PAGE[theme])), 2)
                    self.assertEqual(result['marker']['plain']['danger'], 0)
                    self.assertEqual(result['pill']['info'], 0)

    def test_fringed_text_passes_with_its_marks_and_fails_without_each(self):
        cases = (
            (dict(), None),
            (dict(tint=False), 'the holiday column is not tinted toward danger'),
            (dict(body_tint=False), 'the holiday column body is not tinted toward danger'),
            (dict(marker=False), 'the holiday heading has no danger marker'),
            (dict(pill='info'), 'the CapEx pill is not success green'),
            (dict(pill='text'), 'the CapEx pill is not success green'),
            (dict(cell='info'), 'the filled CapEx cell is not success green'),
            (dict(cell='text'), 'the filled CapEx cell is not success green'),
        )
        for theme in ('dark', 'light'):
            for scale in (1, 2):
                for options, problem in cases:
                    with self.subTest(theme=theme, scale=scale, options=options):
                        grid = SyntheticGrid(theme, scale, fringe=True, **options)
                        if problem is None:
                            result = desktop.grid_marks(grid.frame(), grid.anchors, theme)
                            self.assertTrue(result['passed'])
                            # The plain weekday's fringes are danger-hued too.
                            self.assertGreater(result['marker']['plain']['danger'], 0)
                            self.assertGreater(result['pill']['info'], 0)
                            continue
                        with self.assertRaises(desktop.MissingGridMarks) as raised:
                            desktop.grid_marks(grid.frame(), grid.anchors, theme)
                        self.assertIn(problem, str(raised.exception))

    def test_the_first_real_captures_pass_and_fringes_alone_do_not(self):
        # Measured by run 45574727167 on ubuntu-24.04 (X11) and windows-2025.
        def measured(plain, holiday, pill, cell):
            def counts(values):
                return dict(zip(('danger', 'success', 'info'), values))
            return dict(holidayRednessShift=16, holidayBodyRednessShift=16,
                marker=dict(plain=counts(plain), holiday=counts(holiday)), pill=counts(pill), capexCell=counts(cell))
        ubuntu = measured((50, 1, 48), (88, 0, 0), (0, 97, 32), (0, 87, 26))
        windows = measured((45, 10, 29), (85, 0, 0), (0, 78, 18), (0, 73, 16))
        for name, result in (('ubuntu', ubuntu), ('windows', windows)):
            with self.subTest(capture=name):
                self.assertEqual(desktop.mark_problems(result, 14), [])
                # The holiday heading showing a fringed weekday, as plain does.
                fringed = dict(result, marker=dict(result['marker'], holiday=result['marker']['plain']))
                self.assertEqual(desktop.mark_problems(fringed, 14), ['the holiday heading has no danger marker'])
                # An info-blue pill and cell with green fringes.
                blue = dict(result, pill=dict(danger=0, success=32, info=97), capexCell=dict(danger=0, success=26, info=87))
                self.assertEqual(desktop.mark_problems(blue, 14),
                    ['the CapEx pill is not success green', 'the filled CapEx cell is not success green'])
                # The heading keeps its tint while the cells below lose it.
                bare = dict(result, holidayBodyRednessShift=0)
                self.assertEqual(desktop.mark_problems(bare, 14), ['the holiday column body is not tinted toward danger'])

    def test_each_missing_mark_fails_while_the_text_would_still_read(self):
        cases = (
            (dict(tint=False), 'the holiday column is not tinted toward danger'),
            (dict(body_tint=False), 'the holiday column body is not tinted toward danger'),
            (dict(marker=False), 'the holiday heading has no danger marker'),
            (dict(pill='info'), 'the CapEx pill is not success green'),
            (dict(pill='text'), 'the CapEx pill is not success green'),
            (dict(cell='info'), 'the filled CapEx cell is not success green'),
            (dict(cell='text'), 'the filled CapEx cell is not success green'),
        )
        for theme in ('dark', 'light'):
            for options, problem in cases:
                with self.subTest(theme=theme, options=options):
                    grid = SyntheticGrid(theme, **options)
                    with self.assertRaises(desktop.MissingGridMarks) as raised:
                        desktop.grid_marks(grid.frame(), grid.anchors, theme)
                    self.assertIn(problem, str(raised.exception))
                    self.assertFalse(raised.exception.measurements['passed'])

    def test_hue_classes_follow_the_theme_tokens_not_absolute_rgb(self):
        for theme, tokens in desktop.THEME_MARKS.items():
            for name, color in tokens.items():
                self.assertEqual(desktop.hue_family(color, theme), name)
                # Antialiased against the page the hue holds.
                self.assertEqual(desktop.hue_family(blend(color, SyntheticGrid.PAGE[theme], 0.7), theme), name)
            for grey in ((0x1e, 0x1e, 0x1e), (0xcc, 0xcc, 0xcc), (0x85, 0x85, 0x85)):
                self.assertIsNone(desktop.hue_family(grey, theme))

    def test_headings_that_are_not_adjacent_columns_are_refused(self):
        grid = SyntheticGrid()
        anchors = dict(grid.anchors, **{'15 Oct': dict(grid.anchors['14 Oct'], left=grid.anchors['14 Oct']['left'] + 5)})
        with self.assertRaisesRegex(desktop.MissingGridMarks, 'not adjacent grid columns') as raised:
            desktop.grid_marks(grid.frame(), anchors, 'dark')
        self.assertFalse(raised.exception.measurements['passed'])

    def test_boxes_that_reach_outside_the_capture_are_missing_marks(self):
        grid = SyntheticGrid()
        anchors = dict(grid.anchors, **{'FIX-17': dict(grid.anchors['FIX-17'], top=grid.height * 4)})
        with self.assertRaisesRegex(desktop.MissingGridMarks, 'outside the screenshot'):
            desktop.grid_marks(grid.frame(), anchors, 'dark')

    def test_a_layout_with_misplaced_date_boxes_falls_through_to_the_next_layout(self):
        grid = SyntheticGrid('dark')
        # The first layout reads every label, but its '15 Oct' sits on top of
        # '14 Oct'; the second reads the real boxes.
        plain = grid.anchors['14 Oct']
        words = grid.words()
        holiday = [word for word in words if word['line'] == '1/1/1']
        misplaced = [dict(word, left=plain['left'] + 5 + holiday.index(word) * 20) if word in holiday else word
            for word in words]
        anchors = {label: desktop.settings.ocr.find_label(misplaced, label) for label in ('14 Oct', '15 Oct')}
        self.assertLess(anchors['15 Oct']['left'] - anchors['14 Oct']['left'], 3 * plain['height'])
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'tempo-holiday-capex-dark.png'
            path.write_bytes(png(grid.width, grid.height, bytes(grid.pixels)))
            def run(*args):
                Path(args[-1]).write_bytes(bytes(grid.pixels))
            modes = []
            def recognize(image, mode):
                modes.append(mode)
                return misplaced if len(modes) == 1 else grid.words()
            with patch.object(desktop.settings.ocr, 'analysis_images', return_value=iter((path,))), \
                    patch.object(desktop.settings.ocr, 'recognize', side_effect=recognize):
                result = desktop.check_grid_marks(path, 'dark', run=run)
            self.assertTrue(result['passed'])
            self.assertEqual(modes, [11, 3])
            # Misplaced boxes in every layout fail only after all of them.
            modes.clear()
            with patch.object(desktop.settings.ocr, 'analysis_images', return_value=iter((path,))), \
                    patch.object(desktop.settings.ocr, 'recognize', side_effect=lambda image, mode: modes.append(mode) or misplaced):
                with self.assertRaisesRegex(RuntimeError, 'holiday column and CapEx marks') as raised:
                    desktop.check_grid_marks(path, 'dark', run=run)
            self.assertEqual(modes, [11, 3, 6])
            self.assertIsInstance(raised.exception.__cause__, desktop.MissingGridMarks)

    def test_the_capture_check_scales_ocr_boxes_and_records_its_measurements(self):
        grid = SyntheticGrid('dark', 2)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'tempo-holiday-capex-dark.png'
            path.write_bytes(png(grid.width, grid.height, bytes(grid.pixels)))
            scaled = path.with_suffix('.ocr-scale3.png')
            scaled.write_bytes(png(grid.width // 2, grid.height // 2, b'\0' * (grid.width // 2 * grid.height // 2 * 3)))
            def run(*args):
                Path(args[-1]).write_bytes(bytes(grid.pixels))
            # The first layout misses a heading; the half-size one finds all.
            layouts = {path: [], scaled: grid.words(scale=2)}
            with patch.object(desktop.settings.ocr, 'analysis_images', return_value=iter((path, scaled))), \
                    patch.object(desktop.settings.ocr, 'recognize', side_effect=lambda image, mode: layouts[image]):
                result = desktop.check_grid_marks(path, 'dark', run=run)
            self.assertTrue(result['passed'])
            recorded = json.loads(path.with_suffix('.marks.json').read_text(encoding='utf-8'))
            self.assertTrue(recorded['passed'])
            self.assertEqual(recorded['anchors']['15 Oct']['left'], grid.anchors['15 Oct']['left'])

    def test_a_capture_without_the_marks_fails_the_check(self):
        grid = SyntheticGrid('dark', tint=False, pill='info')
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'tempo-holiday-capex-dark.png'
            path.write_bytes(png(grid.width, grid.height, bytes(grid.pixels)))
            def run(*args):
                Path(args[-1]).write_bytes(bytes(grid.pixels))
            with patch.object(desktop.settings.ocr, 'analysis_images', return_value=iter((path,))), \
                    patch.object(desktop.settings.ocr, 'recognize', return_value=grid.words()):
                with self.assertRaisesRegex(RuntimeError, 'holiday column and CapEx marks') as raised:
                    desktop.check_grid_marks(path, 'dark', run=run)
            self.assertIsInstance(raised.exception.__cause__, desktop.MissingGridMarks)
            self.assertFalse(json.loads(path.with_suffix('.marks.json').read_text(encoding='utf-8'))['passed'])

    @unittest.skipUnless(__import__('shutil').which('ffmpeg'), 'Decoding a real PNG needs ffmpeg')
    def test_ffmpeg_decodes_the_capture_into_the_pixels_it_shows(self):
        grid = SyntheticGrid('light')
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'capture.png'
            path.write_bytes(png(grid.width, grid.height, bytes(grid.pixels)))
            frame = desktop.rgb_frame(path, desktop.settings.run)
        self.assertEqual((frame.width, frame.height), (grid.width, grid.height))
        self.assertEqual(frame.pixels, bytes(grid.pixels))
        self.assertTrue(desktop.grid_marks(frame, grid.anchors, 'light')['passed'])


if __name__ == '__main__':
    unittest.main()
