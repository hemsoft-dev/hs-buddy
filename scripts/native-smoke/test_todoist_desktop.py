"""Owned HTTP tests for installed-app failure scenarios. No GUI or vault access."""
import http.client
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
import urllib.error
import urllib.parse
import urllib.request

SPEC=importlib.util.spec_from_file_location('native_todoist_desktop_tests',Path(__file__).with_name('todoist-desktop.py'))
desktop=importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name]=desktop
SPEC.loader.exec_module(desktop)


class FailureScenarios(unittest.TestCase):
    def setUp(self):
        self.temporary=tempfile.TemporaryDirectory(prefix='buddy-todoist-scenario-')
        self.root=Path(self.temporary.name).resolve()
        assert self.root.parent==Path(tempfile.gettempdir()).resolve()
        self.addCleanup(self.temporary.cleanup)
        config=self.root/'config.json'
        config.write_text('{}')
        self.api=desktop.ScenarioFixture._start_with_host(config,desktop.todoist._Host('true','1',str(self.root)))
        self.addCleanup(self.api.close)
        self.http=urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def get(self,path):
        request=urllib.request.Request(self.api.url+path,headers={'Authorization':'Bearer '+desktop.todoist.TOKEN})
        with self.http.open(request,timeout=3) as response:return json.load(response)

    def sync_read(self, token='*', resources=None):
        body=urllib.parse.urlencode({'sync_token':token,'resource_types':json.dumps(['user'] if resources is None else resources)}).encode()
        request=urllib.request.Request(self.api.url+'sync',data=body,headers={'Authorization':'Bearer '+desktop.todoist.TOKEN})
        with self.http.open(request,timeout=3) as response:return json.load(response)

    def test_navigation_guard_preserves_actual_changed_bytes_even_without_a_command_attempt(self):
        initial=self.api.snapshot()
        self.complete_read()
        desktop.check_read_only_navigation(self.api,initial)
        with self.api.lock:
            row=next(task for task in self.api.tasks if task['id']=='task-today')
            row['content']='Unexpected changed synthetic task'
        with self.assertRaisesRegex(RuntimeError,'changed fixture data'):
            desktop.check_read_only_navigation(self.api,initial)
        self.assertEqual(desktop.posts(self.api),[])
        self.assertEqual(next(task for task in self.api.snapshot()['tasks'] if task['id']=='task-today')['content'],'Unexpected changed synthetic task')
        self.assertNotEqual(initial['tasks'],self.api.snapshot()['tasks'])

    def test_navigation_guard_does_not_accept_a_boolean_replaced_by_an_equal_integer(self):
        initial=self.api.snapshot()
        with self.api.lock:
            row=next(task for task in self.api.tasks if task['id']=='task-today')
            self.assertIs(row['checked'],False)
            row['checked']=0
        with self.assertRaisesRegex(RuntimeError,'changed fixture data'):
            desktop.check_read_only_navigation(self.api,initial)
        self.assertIs(type(next(task for task in self.api.snapshot()['tasks'] if task['id']=='task-today')['checked']),int)
        self.assertEqual(desktop.posts(self.api),[])

    def test_cursor_mutation_skips_an_unseen_row_and_is_reported_by_the_real_sync_delta(self):
        self.api.arm('cursor-change')
        self.complete_read()
        provisional=self.get('tasks?limit=100')
        self.assertFalse(self.api.cursor_change_applied)
        full=self.sync_read()
        marker=self.sync_read(full['sync_token'],['items','projects'])['sync_token']
        first=self.get('tasks?limit=100')
        self.assertTrue(self.api.cursor_change_applied)
        self.assertEqual(first,provisional)
        rows=first['results'][:]
        cursor=first['next_cursor']
        while cursor:
            page=self.get('tasks?'+urllib.parse.urlencode({'limit':'100','cursor':cursor}))
            rows.extend(page['results'])
            cursor=page['next_cursor']
        identifiers=[row['id'] for row in rows]
        self.assertNotIn('task-upcoming',identifiers)
        self.assertEqual(len(identifiers),len(set(identifiers)))
        delta=self.sync_read(marker,['items','projects'])
        self.assertEqual([row['id'] for row in delta['items']],['task-overdue'])
        self.assertTrue(delta['items'][0]['is_deleted'])
        self.assertEqual(desktop.posts(self.api),[])
        self.assertEqual(len([row for row in self.api.snapshot()['traffic'] if row['method']=='POST']),3)

    def complete_read(self):
        self.assertEqual(self.get('user')['id'],'synthetic-todoist-account')
        cursor=None
        while True:
            query={'limit':'100'}
            if cursor:query['cursor']=cursor
            page=self.get('tasks?'+urllib.parse.urlencode(query))
            cursor=page['next_cursor']
            if cursor is None:break
        # The last profile check must still succeed, allowing the application's
        # immutable dataset to publish before the next explicit refresh fails.
        self.assertEqual(self.get('user')['id'],'synthetic-todoist-account')

    def test_offline_scenario_only_disconnects_after_a_complete_read_and_final_identity(self):
        self.api.arm('offline')
        self.complete_read()
        with self.assertRaises(http.client.RemoteDisconnected):self.get('user')
        self.assertEqual(desktop.posts(self.api),[])
        self.assertEqual(self.api.snapshot()['created'],0)
        self.assertEqual(len(self.api.snapshot()['tasks']),5)

    def test_expired_scenario_only_rejects_after_a_complete_read_and_final_identity(self):
        self.api.arm('expired')
        self.complete_read()
        with self.assertRaises(urllib.error.HTTPError) as failure:self.get('user')
        self.assertEqual(failure.exception.code,401)
        failure.exception.close()
        self.assertEqual(desktop.posts(self.api),[])
        self.assertEqual(self.api.snapshot()['created'],0)

    def test_unsupported_scenario_cannot_arm_a_failure(self):
        with self.assertRaises(ValueError):self.api.arm('unknown')
        self.assertEqual(self.get('user')['id'],'synthetic-todoist-account')
        self.assertIsNone(self.api.fail_after_read)

    def test_project_disappears_only_after_its_complete_scoped_read_and_final_identity(self):
        self.api.arm_removed_project('project-empty')
        page=self.get('projects?limit=100&cursor=offset-2')
        self.assertFalse(next(row for row in page['results'] if row['id']=='project-empty')['is_archived'])
        self.assertEqual(self.get('tasks?project_id=project-empty&limit=100')['results'],[])
        self.assertFalse(self.get('projects?limit=100&cursor=offset-2')['results'][0]['is_archived'])
        self.assertEqual(self.get('user')['id'],'synthetic-todoist-account')
        self.assertTrue(self.get('projects?limit=100&cursor=offset-2')['results'][0]['is_archived'])
        self.assertEqual(desktop.posts(self.api),[])

    def test_permission_projects_use_numeric_workspace_ids_from_the_production_wire_shape(self):
        self.api.add_permission_projects()
        rows, cursor = [], None
        while True:
            query={'limit':'100'}
            if cursor:query['cursor']=cursor
            page=self.get('projects?'+urllib.parse.urlencode(query))
            rows.extend(page['results'])
            cursor=page['next_cursor']
            if cursor is None:break
        team=[row for row in rows if row.get('workspace_id') is not None]
        self.assertEqual(len(team),2)
        self.assertTrue(all(type(row['workspace_id']) is int and row['workspace_id']==12345 for row in team))
        self.assertEqual(desktop.posts(self.api),[])

    def test_replacement_task_read_can_fail_after_profile_validation_without_a_write(self):
        self.api.arm('task-read')
        self.complete_read()
        self.assertEqual(self.get('user')['id'],'synthetic-todoist-account')
        with self.assertRaises(urllib.error.HTTPError) as failure:self.get('tasks?limit=100')
        self.assertEqual(failure.exception.code,503)
        failure.exception.close()
        self.assertEqual(len(self.get('tasks?limit=100')['results']),2)
        self.assertEqual(desktop.posts(self.api),[])

    def test_large_account_uses_real_project_limits_and_supports_small_scoped_reads(self):
        self.api.add_large_account()
        rows, cursor = [], None
        while True:
            query = {'limit':'100'}
            if cursor:query['cursor']=cursor
            page = self.get('tasks?'+urllib.parse.urlencode(query))
            rows.extend(page['results'])
            cursor = page['next_cursor']
            if cursor is None:break
        self.assertEqual(len(rows),5006)
        bulk = [row for row in rows if row['id'].startswith('bulk-task-')]
        self.assertEqual(len(bulk),5001)
        from collections import Counter
        self.assertLessEqual(max(Counter(row['project_id'] for row in bulk).values()),300)
        calendar = self.get('tasks/filter?'+urllib.parse.urlencode({'query':'overdue | 7 days','limit':'100'}))
        self.assertEqual(len(calendar['results']),4)
        self.assertIsNone(calendar['next_cursor'])
        project = self.get('tasks?project_id=project-beta&limit=100')
        self.assertTrue(any(row['due'] is None for row in project['results']))
        self.assertEqual(desktop.posts(self.api),[])


if __name__=='__main__':unittest.main()
