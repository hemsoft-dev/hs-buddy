import base64
from http.client import HTTPConnection, RemoteDisconnected
import importlib.util
import json
from pathlib import Path
import socket
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlencode, urlsplit, parse_qs
from urllib.request import build_opener, ProxyHandler, Request
import uuid

spec = importlib.util.spec_from_file_location('tempo_fixture', Path(__file__).with_name('tempo-fixture.py'))
tempo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tempo)


class TempoFixtureTests(unittest.TestCase):
    def test_real_authenticated_identity_uses_only_numeric_loopback_without_hostname_discovery(self):
        with patch.object(socket, 'getfqdn', side_effect=AssertionError('Numeric fixture must not discover local devices')):
            with tempo.TempoFixture() as fixture:
                self.assertEqual(fixture.server.server_address[0], '127.0.0.1')
                self.assertEqual(fixture.server.server_name, '127.0.0.1')
                identity = self.request(fixture, '/rest/api/3/myself', jira=True)
                self.assertEqual(identity, dict(accountId=tempo.ACCOUNT, timeZone='America/New_York'))
                self.assertEqual(fixture.writes, [])
                self.assertEqual(len(fixture.attempts), 1)
                self.assertEqual(fixture.attempts[0]['method'], 'GET')


    def test_request_errors_remain_readable_and_close_when_registered_cleanup_runs(self):
        with tempo.TempoFixture() as fixture:
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/4/accounts?limit=50&offset=0', authorization='Bearer invalid')
            self.assertEqual(error.exception.code, 401)
            self.assertEqual(json.load(error.exception), {'error': 'Synthetic authentication rejected'})
            self.assertTrue(self.doCleanups())
            self.assertTrue(error.exception.closed)
            self.assertEqual(fixture.writes, [])

    def test_editor_issue_project_and_classification_reads_preserve_exact_ids_and_worklogs(self):
        with tempo.TempoFixture() as fixture:
            before = json.dumps(fixture.records, sort_keys=True, ensure_ascii=False)
            project = self.request(fixture, '/rest/api/3/project/FIX', jira=True)
            self.assertEqual(project, dict(id='9007199254740997', key='FIX'))
            path = '/rest/api/3/issue/FIX-17?fields=id,key,customfield_11702,parent'
            for value, expected in [(True, 'Yes'), (False, 'No'), (None, None)]:
                fixture.capitalized = value
                result = self.request(fixture, path, jira=True)
                self.assertEqual(result['id'], str(tempo.ISSUE_ID))
                self.assertEqual(result['fields'].get('customfield_11702', {}).get('value'), expected)
            fixture.current_timezone = 'Pacific/Kiritimati'
            self.assertEqual(self.request(fixture, '/rest/api/3/myself', jira=True)['timeZone'], 'Pacific/Kiritimati')
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/rest/api/3/project/FIX', jira=True, authorization='Basic invalid')
            self.assertEqual(error.exception.code, 401)
            self.assertEqual(json.dumps(fixture.records, sort_keys=True, ensure_ascii=False), before)
            self.assertEqual(fixture.writes, [])
            self.assertTrue(all(item['method'] == 'GET' for item in fixture.attempts))

    def test_complete_project_billing_catalog_retains_status_and_detectable_failure_fixtures(self):
        with tempo.TempoFixture() as fixture:
            before = json.dumps(fixture.records, sort_keys=True, ensure_ascii=False)
            links_path = f'/4/account-links/project/{tempo.PROJECT_ID}?includeGlobalAccounts=true'
            links = self.request(fixture, links_path)
            catalog = self.request(fixture, '/4/accounts?limit=50&offset=0')
            self.assertEqual(links['metadata']['count'], 3)
            self.assertTrue(all(item['scope'] == dict(id=tempo.PROJECT_ID, type='PROJECT') for item in links['results']))
            self.assertEqual([item['default'] for item in links['results']], [True, False, False])
            self.assertEqual([item['id'] for item in catalog['results']], [17, 18, 19])
            self.assertEqual([item['status'] for item in catalog['results']], ['OPEN', 'CLOSED', 'OPEN'])
            self.assertEqual([item['global'] for item in catalog['results']], [False, False, True])
            fixture.missing_billing_account = True
            missing = self.request(fixture, '/4/accounts?limit=50&offset=0')
            self.assertEqual(missing['metadata']['count'], 2)
            self.assertEqual([item['id'] for item in missing['results']], [18, 19])
            fixture.fail_project_accounts = True
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, links_path)
            self.assertEqual(error.exception.code, 503)
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/4/accounts?limit=50&offset=0', authorization='Bearer invalid')
            self.assertEqual(error.exception.code, 401)
            self.assertEqual(json.dumps(fixture.records, sort_keys=True, ensure_ascii=False), before)
            self.assertEqual(fixture.writes, [])

    def request(self, fixture, path, method='GET', value=None, raw=None, jira=False, authorization=None, headers=None):
        token = ('Basic ' + base64.b64encode(f'{tempo.JIRA_EMAIL}:{tempo.JIRA_TOKEN}'.encode()).decode()) if jira else 'Bearer ' + tempo.TEMPO_TOKEN
        data = json.dumps(value, ensure_ascii=False).encode() if value is not None else raw
        request = Request(fixture.url + path, data=data, method=method,
            headers={'Authorization': token if authorization is None else authorization,
                'Content-Type': 'application/json', **(headers or {})})
        try:
            with build_opener(ProxyHandler({})).open(request, timeout=5) as response:
                if response.status == 204:
                    return response.status, response.read()
                return json.load(response)
        except HTTPError as failure:
            self.addCleanup(failure.close)
            raise

    def payload(self):
        return dict(authorAccountId=tempo.ACCOUNT, issueId=tempo.ISSUE_ID,
            startDate='2026-10-16', timeSpentSeconds=90061, description='Synthetic draft 界\nsecond line',
            attributes=[dict(key='_Account_', value='synthetic-billing-17'),
                dict(key='_Unrelated_', value='preserve\n界'), dict(key='_Unset_')])

    def test_create_uses_the_documented_200_success_status(self):
        with tempo.TempoFixture() as fixture:
            request = Request(fixture.url + '/4/worklogs', method='POST',
                data=json.dumps(self.payload()).encode(),
                headers={'Authorization':'Bearer ' + tempo.TEMPO_TOKEN, 'Content-Type':'application/json'})
            with build_opener(ProxyHandler({})).open(request, timeout=5) as response:
                self.assertEqual(response.status, 200)
                self.assertEqual(json.load(response)['author']['accountId'], tempo.ACCOUNT)

    def test_delete_returns_real_empty_204_and_removes_only_its_exact_int64_target(self):
        with tempo.TempoFixture() as fixture:
            path = f'/4/worklogs/{tempo.FIRST_ID}'
            retained = self.request(fixture, f'/4/worklogs/{tempo.FIRST_ID + 1}')
            self.assertEqual(self.request(fixture, path, 'DELETE'), (204, b''))
            self.assertEqual(self.request(fixture, f'/4/worklogs/{tempo.FIRST_ID + 1}'), retained)
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, path)
            self.assertEqual(error.exception.code, 404)
            self.assertEqual(len(fixture.records), 50)
            self.assertEqual(fixture.writes, [dict(method='DELETE', worklogId=tempo.FIRST_ID, bodyBase64='')])
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, path, 'DELETE')
            self.assertEqual(error.exception.code, 404)
            self.assertEqual(sum(row['method'] == 'DELETE' for row in fixture.attempts), 2)
            self.assertEqual(len(fixture.writes), 1)

    def test_delete_refuses_payload_override_authentication_and_foreign_author(self):
        with tempo.TempoFixture() as fixture:
            path = f'/4/worklogs/{tempo.FIRST_ID}'
            for suffix, raw, authorization, status in [
                    ('', b'{}', None, 400),
                    ('?bypassPeriodClosuresAndApprovals=true', None, None, 400),
                    ('', None, 'Bearer synthetic-wrong-token', 401)]:
                with self.assertRaises(HTTPError) as error:
                    self.request(fixture, path + suffix, 'DELETE', raw=raw, authorization=authorization)
                self.assertEqual(error.exception.code, status)
            with fixture.lock:
                fixture.records[tempo.FIRST_ID]['author']['accountId'] = 'foreign-author'
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, path, 'DELETE')
            self.assertEqual(error.exception.code, 403)
            self.assertEqual(len(fixture.records), 51)
            self.assertEqual(fixture.writes, [])
            self.assertEqual(len(fixture.attempts), 4)
            self.assertEqual(base64.b64decode(fixture.attempts[0]['bodyBase64']), b'{}')

    def test_lost_delete_reply_keeps_one_accepted_removal_available_for_read_only_inspection(self):
        with tempo.TempoFixture() as fixture:
            path = f'/4/worklogs/{tempo.FIRST_ID}'
            fixture.drop_next_delete = True
            with self.assertRaises(RemoteDisconnected):
                self.request(fixture, path, 'DELETE')
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, path)
            self.assertEqual(error.exception.code, 404)
            self.assertEqual(len(fixture.records), 50)
            self.assertEqual(len(fixture.writes), 1)
            self.assertEqual(sum(row['method'] == 'DELETE' for row in fixture.attempts), 1)

    def test_numeric_loopback_marker_and_jira_identity_keep_exact_large_issue_id(self):
        with tempo.TempoFixture() as fixture:
            marker = fixture.marker()
            self.assertEqual(set(marker), {'version', 'nonce', 'tempo_url', 'jira_url', 'now_utc'})
            self.assertEqual(uuid.UUID(marker['nonce']).version, 4)
            self.assertEqual(urlsplit(marker['tempo_url']).hostname, '127.0.0.1')
            self.assertEqual(marker['jira_url'], fixture.url + '/')
            self.assertEqual(marker['now_utc'], tempo.NOW)
            self.assertEqual(self.request(fixture, '/rest/api/3/myself', jira=True),
                dict(accountId=tempo.ACCOUNT, timeZone='America/New_York'))
            issue = self.request(fixture, '/rest/api/3/issue/FIX-17?fields=id,key,summary', jira=True)
            self.assertEqual(issue['id'], '9007199254740995')
            fixture.fail_jira = True
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/rest/api/3/myself', jira=True)
            self.assertEqual(error.exception.code, 401)
            self.assertEqual(len(fixture.attempts), 3)
            self.assertEqual(fixture.writes, [])

    def test_pagination_scope_and_schedule_keep_all_rows_and_known_zero_days(self):
        with tempo.TempoFixture() as fixture:
            scope = dict(from_='2026-10-15', to='2026-10-18', limit=50, offset=0)
            scope['from'] = scope.pop('from_')
            path = '/4/worklogs/user/' + tempo.ACCOUNT
            first = self.request(fixture, path + '?' + urlencode(scope))
            self.assertEqual(len(first['results']), 50)
            following = urlsplit(first['metadata']['next'])
            self.assertEqual(following.hostname, 'api.tempo.io')
            self.assertEqual(parse_qs(following.query)['offset'], ['50'])
            second = self.request(fixture, path + '?' + following.query)
            self.assertEqual(len(second['results']), 1)
            self.assertIsNone(second['metadata']['next'])
            identifiers = [row['tempoWorklogId'] for row in first['results'] + second['results']]
            self.assertEqual(identifiers, list(range(tempo.FIRST_ID, tempo.FIRST_ID + 51)))
            self.assertNotIn('values', first['results'][0]['attributes'])
            schedule = self.request(fixture, '/4/user-schedule/' + tempo.ACCOUNT + '?from=2026-10-15&to=2026-10-18')
            self.assertEqual([(row['requiredSeconds'], row['type']) for row in schedule['results']],
                [(0, 'HOLIDAY'), (28800, 'WORKING_DAY'), (0, 'NON_WORKING_DAY'), (0, 'NON_WORKING_DAY')])
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/4/worklogs/user/foreign?from=2026-10-15&to=2026-10-18&limit=50&offset=0')
            self.assertEqual(error.exception.code, 404)
            self.assertEqual(fixture.writes, [])

    def test_update_expanded_attributes_and_exact_request_bytes_survive_readback(self):
        with tempo.TempoFixture() as fixture:
            path = f'/4/worklogs/{tempo.FIRST_ID}'
            attributes = self.request(fixture, path + '/work-attribute-values')
            self.assertEqual(attributes['metadata']['count'], 3)
            value = self.payload()
            value.pop('issueId')
            value['billableSeconds'] = 0
            value['startTime'] = '10:11:12'
            raw = json.dumps(value, ensure_ascii=False, indent=2).encode()
            result = self.request(fixture, path, 'PUT', raw=raw)
            self.assertEqual(result['tempoWorklogId'], tempo.FIRST_ID)
            self.assertEqual(result['issue']['id'], tempo.ISSUE_ID)
            self.assertEqual(result['timeSpentSeconds'], 90061)
            self.assertEqual(result['attributes']['values'], attributes['results'])
            self.assertEqual(base64.b64decode(fixture.writes[0]['bodyBase64']), raw)
            self.assertEqual(self.request(fixture, path)['description'], value['description'])
            self.assertEqual(self.request(fixture, path + '/work-attribute-values')['results'], value['attributes'])
            self.assertEqual(sum(row['method'] == 'PUT' for row in fixture.attempts), 1)

    def test_invalid_read_queries_return_static_errors_and_preserve_service_state(self):
        queries = ['to=2026-10-18&limit=50&offset=0',
            'from=2026-10-15&from=2026-10-16&to=2026-10-18&limit=50&offset=0',
            'from=2026-10-15&to=2026-10-18&limit=2&offset=0',
            'from=2026-10-15&to=2026-10-18&limit=50&offset=-1',
            'from=2026-10-15&to=2026-10-14&limit=50&offset=0',
            'from=2026-10-15&to=2028-10-18&limit=50&offset=0']
        with tempo.TempoFixture() as fixture:
            for query in queries:
                with self.subTest(query=query):
                    with self.assertRaises(HTTPError) as error:
                        self.request(fixture, '/4/worklogs/user/' + tempo.ACCOUNT + '?' + query)
                    self.assertEqual(error.exception.code, 400)
                    self.assertEqual(json.load(error.exception), {'error': 'Synthetic request rejected'})
            self.assertEqual(self.request(fixture, '/rest/api/3/myself', jira=True)['accountId'], tempo.ACCOUNT)
            self.assertEqual(len(fixture.records), 51)
            self.assertEqual(len(fixture.attempts), len(queries) + 1)
            self.assertEqual(fixture.writes, [])

    def test_authentication_rejection_consumes_body_and_counts_every_write_attempt(self):
        with tempo.TempoFixture() as fixture:
            raw = b'Synthetic rejected body ' * 4096
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/4/worklogs', 'POST', raw=raw, authorization='Bearer synthetic-wrong-token')
            self.assertEqual(error.exception.code, 401)
            self.assertEqual(len(fixture.attempts), 1)
            self.assertEqual(base64.b64decode(fixture.attempts[0]['bodyBase64']), raw)
            self.assertEqual(fixture.writes, [])
            self.assertEqual(len(fixture.records), 51)
            self.assertNotIn('Authorization', json.dumps(fixture.attempts))

    def test_dropped_create_reply_applies_once_and_repeated_uuid_never_hides_a_resend(self):
        with tempo.TempoFixture() as fixture:
            value = self.payload()
            headers = {'X-Request-Id': str(uuid.uuid4())}
            fixture.drop_next_create = True
            with self.assertRaises(RemoteDisconnected):
                self.request(fixture, '/4/worklogs', 'POST', value=value, headers=headers)
            self.assertEqual(len(fixture.writes), 1)
            self.assertEqual(len(fixture.records), 52)
            retained = self.request(fixture, '/4/worklogs/' + str(fixture.writes[0]['worklogId']))
            self.assertEqual(retained['description'], value['description'])
            second = self.request(fixture, '/4/worklogs', 'POST', value=value, headers=headers)
            self.assertEqual(len(fixture.writes), 2)
            self.assertEqual(sum(row['method'] == 'POST' for row in fixture.attempts), 2)
            self.assertNotEqual(second['tempoWorklogId'], retained['tempoWorklogId'])
            self.assertEqual(fixture.writes[0]['bodyBase64'], fixture.writes[1]['bodyBase64'])

    def test_successful_acknowledgement_survives_fixture_refresh_failure(self):
        with tempo.TempoFixture() as fixture:
            fixture.fail_reads_after_write = True
            path = f'/4/worklogs/{tempo.FIRST_ID}'
            self.assertEqual(self.request(fixture, path)['tempoWorklogId'], tempo.FIRST_ID)
            value = self.payload()
            acknowledgement = self.request(fixture, '/4/worklogs', 'POST', value=value)
            self.assertEqual(acknowledgement['description'], value['description'])
            created = '/4/worklogs/' + str(acknowledgement['tempoWorklogId'])
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, created)
            self.assertEqual(error.exception.code, 503)
            self.assertEqual(len(fixture.writes), 1)
            self.assertEqual(sum(row['method'] == 'POST' for row in fixture.attempts), 1)
            self.assertEqual(len(fixture.records), 52)
            self.assertEqual(fixture.records[acknowledgement['tempoWorklogId']]['description'], value['description'])
            fixture.fail_reads_after_write = False
            self.assertEqual(self.request(fixture, created)['tempoWorklogId'], acknowledgement['tempoWorklogId'])
            self.assertEqual(len(fixture.writes), 1)

    def test_invalid_scope_admin_override_or_duplicate_fields_never_mutate_fixture(self):
        for changed in (dict(authorAccountId='foreign'), dict(timeSpentSeconds=True),
                dict(issueId='9007199254740995'), dict(bypassPeriodClosuresAndApprovals=True),
                dict(idempotencyKey='invented'), dict(attributes=[dict(key='same'), dict(key='same')])):
            with self.subTest(changed=changed), tempo.TempoFixture() as fixture:
                value = {**self.payload(), **changed}
                with self.assertRaises(HTTPError) as error:
                    self.request(fixture, '/4/worklogs', 'POST', value=value)
                self.assertEqual(error.exception.code, 400)
                self.assertEqual(len(fixture.attempts), 1)
                self.assertEqual(fixture.writes, [])
        with tempo.TempoFixture() as fixture:
            raw = json.dumps(self.payload()).replace('"timeSpentSeconds": 90061', '"timeSpentSeconds": 1, "timeSpentSeconds": 90061').encode()
            with self.assertRaises(HTTPError) as error:
                self.request(fixture, '/4/worklogs', 'POST', raw=raw)
            self.assertEqual(error.exception.code, 400)
            self.assertEqual(fixture.writes, [])

    def test_oversized_declared_body_is_bounded_and_cleanup_closes_only_owned_port(self):
        with tempo.TempoFixture() as fixture:
            port = fixture.server.server_port
            connection = HTTPConnection('127.0.0.1', port, timeout=5)
            connection.putrequest('POST', '/4/worklogs')
            connection.putheader('Content-Length', str(tempo.MAX_BODY + 1))
            connection.endheaders()
            response = connection.getresponse()
            self.assertEqual(response.status, 413)
            response.read()
            connection.close()
            self.assertEqual(len(fixture.attempts), 1)
            self.assertEqual(fixture.writes, [])
        self.assertFalse(fixture.thread.is_alive())
        with socket.socket() as probe:
            self.assertNotEqual(probe.connect_ex(('127.0.0.1', port)), 0)


if __name__ == '__main__':
    unittest.main()
