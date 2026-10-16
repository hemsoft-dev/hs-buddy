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
