import importlib.util
import json
import os
from pathlib import Path
import unittest
from tempfile import TemporaryDirectory
from unittest.mock import patch
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('copilot_desktop', Path(__file__).with_name('copilot-desktop.py'))
copilot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(copilot)

class CopilotQualificationTests(unittest.TestCase):
    def test_streaming_capture_handshake_is_scoped_and_restored_after_failure(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ, {'BUDDY_COPILOT_STREAM_CAPTURE': 'previous'}):
            output = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, 'owned driver failed'):
                with copilot.streaming_capture(output) as capture:
                    self.assertEqual(capture, output / 'chat-dark' / 'streaming-dark.png')
                    self.assertEqual(os.environ['BUDDY_COPILOT_STREAM_CAPTURE'], str(capture))
                    raise RuntimeError('owned driver failed')
            self.assertEqual(os.environ['BUDDY_COPILOT_STREAM_CAPTURE'], 'previous')

    def test_existing_streaming_capture_is_preserved_and_cannot_release_a_new_stream(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            capture = output / 'chat-dark' / 'streaming-dark.png'
            capture.parent.mkdir()
            capture.write_bytes(b'previous evidence')
            with self.assertRaisesRegex(RuntimeError, 'Preserve the existing streaming capture'):
                with copilot.streaming_capture(output):
                    self.fail('An old image cannot acknowledge a new stream')
            self.assertEqual(capture.read_bytes(), b'previous evidence')

    def test_saved_status_does_not_prove_its_offscreen_controls_are_visible(self):
        class ClippedConversationDriver:
            def __init__(self):
                self.visible = set()
                self.clicked = []

            def drive(self, name, actions, record=False):
                for action in actions:
                    if action == ('wait_scroll_text', 'Saved to results'):
                        # The captured Linux viewport shows this status at the
                        # bottom while both following buttons remain clipped.
                        self.visible = {'Saved to results'}
                    elif action[0] == 'reveal_text' and action[1] in ('Copy answer', 'Inspect saved result'):
                        if action[2] not in self.visible:
                            raise AssertionError('Scroll anchor is not in the visible conversation')
                        self.visible.add(action[1])
                    elif action[0] == 'click' and action[1] in ('Copy answer', 'Inspect saved result'):
                        if action[1] not in self.visible:
                            raise AssertionError('Saved status did not reveal the requested control: ' + action[1])
                        self.clicked.append(action[1])
                return Path('unused-synthetic-conversation-output')

        for flow, expected in ((copilot.chat_turns, ['Copy answer', 'Inspect saved result']),
                (copilot.review_pull_request, ['Inspect saved result'])):
            with self.subTest(flow=flow.__name__):
                driver = ClippedConversationDriver()
                with patch.object(copilot.github, 'check'), patch.object(copilot.github, 'check_column'):
                    flow(driver)
                self.assertEqual(driver.clicked, expected)

    def test_fresh_review_selects_the_offline_model_before_dispatch(self):
        # The actual failed Mac capture starts a fresh review app with this
        # configured model. Chat's earlier, view-local choice does not carry over.
        class FreshReviewDriver:
            def __init__(self):
                self.model = 'claude-sonnet-4.5'
                self.catalog = False
                self.context = False
                self.dispatched = False

            def drive(self, name, actions):
                self.assert_name = name
                for action in actions:
                    if action == ('click', 'Review with Buddy assistant'):
                        self.context = True
                    elif action == ('click', 'Load available models'):
                        self.catalog = True
                    elif action == ('wait_models', 'Synthetic offline model'):
                        if not self.catalog:
                            raise AssertionError('Model discovery was not requested')
                    elif action == ('click', 'Synthetic offline model'):
                        if not self.catalog:
                            raise AssertionError('Cannot select a model before discovery')
                        self.model = 'gpt-4.1'
                    elif action == ('click', 'Run and save'):
                        if not self.context or self.model != 'gpt-4.1':
                            raise AssertionError('Fresh review dispatch reaches the offline identity guard with model=' + self.model)
                        self.dispatched = True
                return Path('unused-synthetic-review-output')

        driver = FreshReviewDriver()
        with patch.object(copilot.github, 'check') as screenshots, \
                patch.object(copilot.github, 'check_column') as review_notice:
            copilot.review_pull_request(driver)
        self.assertEqual(driver.assert_name, 'review-dark')
        self.assertTrue(driver.dispatched)
        self.assertEqual(driver.model, 'gpt-4.1')
        self.assertEqual(screenshots.call_count, 1)
        review_notice.assert_called_once_with(Path('unused-synthetic-review-output') / 'review-result-dark.png',
            'Captured review', 'Captured review', 'Freshness has not been checked')

    def test_privacy_gate_requires_app_logs_and_rejects_private_transport_content(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            with self.assertRaisesRegex(RuntimeError, 'not captured'):
                copilot.assert_private_content_not_logged(output)
            case = output / 'synthetic-case'
            case.mkdir()
            log = case / 'app.log'
            log.write_text('static application warning')
            copilot.assert_private_content_not_logged(output)
            log.write_text('received payload BUDDYSTREAM61527')
            with self.assertRaisesRegex(RuntimeError, 'Private synthetic'):
                copilot.assert_private_content_not_logged(output)
    def test_non_ci_invocation_stops_before_argument_parsing_backend_or_ui(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false', 'BUDDY_SETTINGS_FIXTURE': '1'}), \
                patch.object(copilot.argparse.ArgumentParser, 'parse_args') as arguments, \
                patch.object(copilot.settings, 'SettingsBackend') as backend, \
                patch.object(copilot.settings, 'Driver') as driver:
            with self.assertRaisesRegex(RuntimeError, 'disposable GitHub Actions'):
                copilot.main()
            arguments.assert_not_called()
            backend.assert_not_called()
            driver.assert_not_called()

    def test_process_inspection_is_also_rejected_outside_ci(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false'}), patch.object(copilot.subprocess, 'check_output') as inspect:
            with self.assertRaisesRegex(RuntimeError, 'disposable CI'):
                copilot.assert_no_owned_children(Path('unused-owned-root'))
            inspect.assert_not_called()

    def request(self, api, path, body=None):
        request = Request(api.url.rstrip('/') + path, data=None if body is None else json.dumps(body).encode(),
            headers={'Authorization': 'Bearer native-fixture-synthetic-account', 'Content-Type': 'application/json'})
        with urlopen(request, timeout=2) as response:
            return json.load(response)

    def test_real_loopback_usage_budget_and_review_responses_stay_synthetic(self):
        with copilot.CopilotGitHubBackend() as api:
            usage = self.request(api, '/orgs/fixture-org/settings/billing/usage?year=2026&month=10')
            self.assertEqual(usage['usageItems'][0]['date'], '2026-10-02')
            self.assertEqual(usage['usageItems'][0]['quantity'], 1200)
            self.assertEqual(self.request(api, '/orgs/fixture-org/copilot/billing')['seat_breakdown']['total'], 1)
            self.assertEqual(self.request(api, '/organizations/fixture-org/settings/billing/budgets?page=1')['budgets'], [])
            first = self.request(api, '/enterprises/fixture-enterprise/settings/billing/budgets?page=1')
            second = self.request(api, '/enterprises/fixture-enterprise/settings/billing/budgets?page=2')
            self.assertTrue(first['has_next_page'])
            self.assertFalse(second['has_next_page'])
            self.assertEqual(second['budgets'][0]['budget_amount'], 200)
            pull = self.request(api, '/graphql', {'query': 'query NativePull($number:Int!){repository{pullRequest{body}}}', 'variables': {'number': 7}})
            self.assertIn('BUDDYSTREAM61527', pull['data']['repository']['pullRequest']['body'])
            self.assertTrue(all(request['account'] == 'synthetic-account' for request in api.requests))
            self.assertNotIn('Authorization', json.dumps(api.requests))

if __name__ == '__main__':
    unittest.main()
