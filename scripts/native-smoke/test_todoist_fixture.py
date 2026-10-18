"""Headless owned-loopback tests; no process-environment or upstream access."""
import copy
import http.client
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
import unittest.mock
import urllib.error
import urllib.parse
import urllib.request
import uuid

SPEC = importlib.util.spec_from_file_location('native_todoist_fixture', Path(__file__).with_name('todoist-fixture.py'))
fixture = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = fixture
SPEC.loader.exec_module(fixture)


class LoopbackDiscoveryTests(unittest.TestCase):
    def test_shared_fixture_server_refuses_non_loopback_before_binding(self):
        for host in ('0.0.0.0', 'localhost', '192.168.1.2'):
            with self.subTest(host=host), unittest.mock.patch('socketserver.TCPServer.server_bind') as bind:
                with self.assertRaisesRegex(ValueError, 'numeric IPv4 loopback'):
                    fixture.LoopbackHTTPServer((host, 0), fixture._Handler)
                bind.assert_not_called()

    def test_real_todoist_requests_never_discover_a_local_hostname(self):
        with tempfile.TemporaryDirectory(prefix='buddy-no-discovery-') as temporary:
            root = Path(temporary).resolve()
            config = root / 'config.json'
            config.write_text('{}', encoding='utf-8')
            with unittest.mock.patch.object(fixture.socket, 'getfqdn', side_effect=AssertionError('Numeric loopback must not discover local devices')):
                api = fixture.Fixture._start_with_host(config, fixture._Host('true', '1', str(root)))
                try:
                    self.assertEqual(api.server.server_address[0], '127.0.0.1')
                    self.assertEqual(api.server.server_name, '127.0.0.1')
                    request = urllib.request.Request(api.url + 'user', headers={'Authorization': 'Bearer ' + fixture.TOKEN})
                    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                    with opener.open(request, timeout=3) as response:
                        self.assertEqual(json.load(response)['id'], 'synthetic-todoist-account')
                    self.assertEqual(api.snapshot()['traffic'], [{'method':'GET','path':'/api/v1/user'}])
                finally:
                    api.close()
            self.assertFalse((root / fixture.MARKER).exists())


class TodoistFixtureTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='buddy-owned-todoist-test-')
        self.root = Path(self.temporary.name).resolve()
        assert self.root.parent == Path(tempfile.gettempdir()).resolve()
        self.addCleanup(self.temporary.cleanup)
        self.config = self.root / 'config.json'
        self.config.write_text('{}')
        self.host = fixture._Host('true', '1', str(self.root))
        self.api = fixture.Fixture._start_with_host(self.config, self.host)
        self.addCleanup(self.api.close)
        self.http = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(self, route, commands=None, token=fixture.TOKEN):
        data = None if commands is None else urllib.parse.urlencode({'commands': json.dumps(commands)}).encode()
        request = urllib.request.Request(self.api.url + route, data=data,
            headers={'Authorization': f'Bearer {token}'})
        with self.http.open(request, timeout=3) as response:
            return json.load(response)

    def sync_read(self, token='*', resources=None, extra=None, authorization=fixture.TOKEN):
        form = {'sync_token':token, 'resource_types':json.dumps(['user'] if resources is None else resources), **(extra or {})}
        request = urllib.request.Request(self.api.url+'sync', data=urllib.parse.urlencode(form).encode(),
            headers={'Authorization':f'Bearer {authorization}'})
        with self.http.open(request, timeout=3) as response:return json.load(response)

    def test_read_only_sync_tracks_real_task_and_project_changes_without_sending_commands(self):
        original = self.api.snapshot()
        full = self.sync_read()
        self.assertTrue(full['full_sync'])
        self.assertEqual(full['user']['id'], 'synthetic-todoist-account')
        marker = full['sync_token']
        unchanged = self.sync_read(marker, ['items','projects'])
        self.assertFalse(unchanged['full_sync'])
        self.assertEqual((unchanged['items'],unchanged['projects']),([],[]))
        with self.api.lock:
            self.api.tasks[2]['content'] = 'Synthetic collaborator changed task'
            self.api.projects[1]['name'] = 'Synthetic collaborator changed project'
        delta = self.sync_read(marker, ['items','projects'])
        self.assertEqual([row['id'] for row in delta['items']], ['task-upcoming'])
        self.assertEqual([row['id'] for row in delta['projects']], ['project-beta'])
        self.assertNotEqual(delta['sync_token'],marker)
        state = self.api.snapshot()
        self.assertEqual(state['created'],0)
        self.assertEqual(len(state['traffic']),3)
        self.assertTrue(all(row.get('syncRead') is True for row in state['traffic']))
        self.assertTrue(all('commands' not in row for row in state['requests']))
        self.assertEqual(state['tasks'][0],original['tasks'][0])

    def test_read_only_sync_never_consumes_the_next_command_acknowledgement_failure(self):
        self.api.drop_next_ack=True
        self.sync_read()
        self.assertTrue(self.api.drop_next_ack)
        command=self.command(content='Synthetic lost reply after resource read',project_id='project-empty')
        with self.assertRaises((http.client.RemoteDisconnected,urllib.error.URLError)):
            self.request('sync',[command])
        state=self.api.snapshot()
        self.assertEqual(state['created'],1)
        self.assertEqual(len([row for row in state['traffic'] if row['method']=='POST' and not row.get('syncRead',False)]),1)
        self.assertEqual(len([row for row in state['requests'] if 'commands' in row]),1)

    def test_a_command_cannot_be_hidden_in_the_read_only_sync_audit(self):
        before=self.api.snapshot()['tasks']
        command=self.command('item_delete',id='task-today')
        with self.assertRaises(urllib.error.HTTPError) as error:
            self.sync_read(extra={'commands':json.dumps([command])})
        self.assertEqual(error.exception.code,400)
        state=self.api.snapshot()
        self.assertEqual(state['tasks'],before)
        self.assertFalse(state['traffic'][0].get('syncRead',False))
        self.assertEqual(state['requests'],[])

    @staticmethod
    def command(kind='item_add', **arguments):
        value = dict(type=kind, uuid=str(uuid.uuid4()), args=arguments)
        if kind == 'item_add':
            value['temp_id'] = str(uuid.uuid4())
        return value

    def all_tasks(self, project=None):
        rows, cursor = [], None
        while True:
            query = dict(limit='100')
            if project:
                query['project_id'] = project
            if cursor:
                query['cursor'] = cursor
            page = self.request('tasks?' + urllib.parse.urlencode(query))
            rows.extend(page['results'])
            cursor = page['next_cursor']
            if cursor is None:
                return rows

    def test_marker_has_owned_numeric_origin_and_explicit_fixed_clock(self):
        marker = json.loads((self.root / fixture.MARKER).read_bytes())
        self.assertEqual(set(marker), {'version', 'nonce', 'api_url', 'now_utc'})
        self.assertEqual(marker['api_url'], self.api.url)
        self.assertEqual(marker['now_utc'], '2026-11-02T05:30:00Z')
        self.assertEqual(uuid.UUID(marker['nonce']).version, 4)
        self.assertEqual(self.request('user')['tz_info']['timezone'], 'America/Winnipeg')

    def test_calendar_filter_excludes_undated_tasks_and_obeys_its_cursor(self):
        rows, cursor = [], None
        while True:
            query = {'query':'overdue | 7 days','limit':'100'}
            if cursor: query['cursor'] = cursor
            page = self.request('tasks/filter?' + urllib.parse.urlencode(query))
            rows.extend(page['results'])
            cursor = page['next_cursor']
            if cursor is None: break
        self.assertEqual({row['id'] for row in rows}, {'task-overdue','task-today','task-upcoming','task-recurring'})
        self.assertEqual(self.api.snapshot()['created'], 0)

    def test_project_permissions_reject_forbidden_commands_without_mutating_tasks(self):
        self.api.projects[0].update(role='COMPLETE_ONLY',workspace_id='team')
        before = self.api.snapshot()['tasks']
        for command in [self.command(content='Forbidden'), self.command('item_update',id='task-today',content='Forbidden'),
                        self.command('item_delete',id='task-today')]:
            reply = self.request('sync',[command])
            self.assertEqual(reply['sync_status'][command['uuid']]['error_code'],403)
            self.assertEqual(self.api.snapshot()['tasks'],before)
        command = self.command('item_close',id='task-today')
        self.assertEqual(self.request('sync',[command])['sync_status'][command['uuid']],'ok')
        self.assertTrue(next(row for row in self.api.snapshot()['tasks'] if row['id']=='task-today')['checked'])

    def test_network_disconnect_is_an_actual_lost_connection_and_records_no_write(self):
        self.api.disconnect = True
        with self.assertRaises(http.client.RemoteDisconnected):
            self.request('user')
        self.assertEqual(self.api.snapshot()['traffic'], [{'method':'GET','path':'/api/v1/user'}])
        self.assertEqual(self.api.snapshot()['requests'], [])
        self.assertEqual(self.api.snapshot()['created'], 0)

    def test_one_shot_authentication_failure_can_be_rechecked_without_changing_credentials(self):
        self.api.reject_next_auth = True
        with self.assertRaises(urllib.error.HTTPError) as error:
            self.request('user')
        self.assertEqual(error.exception.code, 401)
        error.exception.close()
        self.assertEqual(self.request('user')['id'], 'synthetic-todoist-account')
        self.assertEqual(self.api.snapshot()['traffic'], [{'method':'GET','path':'/api/v1/user'}]*2)
        self.assertEqual(self.api.snapshot()['created'], 0)

    def test_disabled_or_foreign_profiles_fail_before_opening_another_server(self):
        for host, path in [(fixture._Host('false', '1', str(self.root)), self.config),
                           (fixture._Host('true', None, str(self.root)), self.config),
                           (fixture._Host('true', '1', None), self.config),
                           (self.host, Path('relative.json')),
                           (self.host, self.root / 'missing-config.json'),
                           (fixture._Host('true', '1', str(self.root / 'elsewhere')), self.config)]:
            with self.subTest(host=host, path=path), unittest.mock.patch.object(fixture, 'LoopbackHTTPServer') as server:
                with self.assertRaises(RuntimeError):
                    fixture.Fixture._start_with_host(path, host)
                server.assert_not_called()

    def test_existing_marker_and_replacement_during_cleanup_are_preserved(self):
        marker = self.root / fixture.MARKER
        original = marker.read_bytes()
        with self.assertRaises(RuntimeError):
            fixture.Fixture._start_with_host(self.config, self.host)
        self.assertEqual(marker.read_bytes(), original)
        changed = b'{"synthetic":"another fixture owns this marker"}'
        marker.write_bytes(changed)
        self.api.close()
        self.assertEqual(marker.read_bytes(), changed)

    def test_authentication_and_service_failures_do_not_return_cached_profile(self):
        self.assertEqual(self.request('user')['id'], 'synthetic-todoist-account')
        for field, status in [('expired', 401), ('unavailable', 503)]:
            setattr(self.api, field, True)
            with self.assertRaises(urllib.error.HTTPError) as error:
                self.request('user')
            self.assertEqual(error.exception.code, status)
            setattr(self.api, field, False)
        with self.assertRaises(urllib.error.HTTPError) as error:
            self.request('user', token='synthetic-wrong-token')
        self.assertEqual(error.exception.code, 401)

    def test_pagination_and_project_scope_retain_all_dates_and_undated_tasks(self):
        rows = self.all_tasks()
        self.assertEqual(len(rows), 5)
        self.assertEqual(len({row['id'] for row in rows}), 5)
        self.assertEqual(len(self.all_tasks('project-alpha')), 3)
        self.assertEqual(self.all_tasks('project-empty'), [])
        self.assertTrue(any(row['due'] is None for row in rows))
        self.assertEqual(next(row for row in rows if row['id'] == 'task-today')['priority'], 4)
        with self.assertRaises(urllib.error.HTTPError) as error:
            self.request('tasks?cursor=https%3A%2F%2Fexample.invalid')
        self.assertEqual(error.exception.code, 400)

    def test_create_edit_complete_reopen_and_delete_change_readback(self):
        command = self.command(content='Synthetic created task 界', description='Retained description',
            priority=4, due=dict(date='2026-11-04'), project_id='project-empty', auto_parse_labels=False)
        reply = self.request('sync', [command])
        identifier = reply['temp_id_mapping'][command['temp_id']]
        self.assertEqual(reply['sync_status'][command['uuid']], 'ok')
        row, = self.all_tasks('project-empty')
        self.assertEqual((row['id'], row['due']['date']), (identifier, '2026-11-04'))
        self.request('sync', [self.command('item_update', id=identifier, content='Synthetic edited task')])
        edited, = self.all_tasks('project-empty')
        self.assertEqual((edited['content'], edited['description'], edited['priority']),
            ('Synthetic edited task', 'Retained description', 4))
        self.request('sync', [self.command('item_close', id=identifier)])
        self.assertEqual(self.all_tasks('project-empty'), [])
        self.request('sync', [self.command('item_uncomplete', id=identifier)])
        self.assertEqual(len(self.all_tasks('project-empty')), 1)
        self.request('sync', [self.command('item_delete', id=identifier)])
        self.assertEqual(self.all_tasks('project-empty'), [])

    def test_dropped_ack_applies_once_and_exact_original_uuid_retry_recovers_ack(self):
        command = self.command(content='Synthetic ambiguous create', project_id='project-empty')
        self.api.drop_next_ack = True
        with self.assertRaises((http.client.RemoteDisconnected, urllib.error.URLError)):
            self.request('sync', [command])
        state = self.api.snapshot()
        self.assertEqual(state['created'], 1)
        self.assertEqual(len([row for row in state['tasks'] if row['content'] == 'Synthetic ambiguous create']), 1)
        reply = self.request('sync', [command])
        self.assertEqual(reply['sync_status'][command['uuid']], 'ok')
        self.assertEqual(self.api.snapshot()['created'], 1)
        sent = [row['commands'][0] for row in self.api.snapshot()['requests'] if row['method'] == 'POST']
        self.assertEqual(sent, [command, command])

    def test_reused_uuid_cannot_replace_payload_and_recurrence_is_server_owned(self):
        command = self.command('item_close', id='task-recurring')
        self.request('sync', [command])
        self.request('sync', [command])
        row = next(row for row in self.all_tasks() if row['id'] == 'task-recurring')
        self.assertEqual(row['due']['date'], '2026-11-03')
        changed = copy.deepcopy(command)
        changed['args']['id'] = 'task-today'
        self.assertEqual(self.request('sync', [changed])['sync_status'][command['uuid']], 'ok')
        self.assertTrue(any(row['id'] == 'task-today' for row in self.all_tasks()))

    def test_malformed_and_oversized_requests_cannot_change_service_state(self):
        before = self.api.snapshot()['tasks']
        command = self.command(content='Synthetic invalid date', due=dict(string='missing date'))
        with self.assertRaises(urllib.error.HTTPError) as error:
            self.request('sync', [command])
        self.assertEqual(error.exception.code, 400)
        self.assertEqual(self.api.snapshot()['created'], 0)
        self.assertEqual(self.api.snapshot()['tasks'], before)

        with self.assertRaises(urllib.error.HTTPError) as error:
            self.request('tasks?limit=1&limit=2')
        self.assertEqual(error.exception.code, 400)
        address = urllib.parse.urlsplit(self.api.url)
        connection = http.client.HTTPConnection(address.hostname, address.port, timeout=3)
        try:
            connection.request('POST', '/api/v1/sync', body=b'', headers={
                'Authorization': f'Bearer {fixture.TOKEN}', 'Content-Length': str(fixture.MAX_BODY + 1)})
            response = connection.getresponse()
            self.assertEqual(response.status, 400)
            self.assertEqual(json.load(response), dict(error='Unsupported synthetic request'))
        finally:
            connection.close()
        self.assertEqual(self.api.snapshot()['tasks'], before)

    def test_request_audit_counts_rejected_auth_without_credentials_or_command_bodies(self):
        self.api.expired = True
        for route, commands in [('user', None), ('sync', [self.command(content='Synthetic rejected change')])]:
            with self.assertRaises(urllib.error.HTTPError) as error:
                self.request(route, commands)
            self.assertEqual(error.exception.code, 401)
        state = self.api.snapshot()
        self.assertEqual(state['traffic'], [dict(method='GET', path='/api/v1/user'),
                                            dict(method='POST', path='/api/v1/sync')])
        self.assertEqual(state['requests'], [])
        self.assertEqual(state['created'], 0)
        self.assertNotIn(fixture.TOKEN, json.dumps(state['traffic']))
        self.assertNotIn('Synthetic rejected change', json.dumps(state['traffic']))


if __name__ == '__main__':
    unittest.main()
