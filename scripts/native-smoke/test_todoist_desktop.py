"""Owned HTTP tests for installed-app failure scenarios. No GUI or vault access."""
import http.client
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch
import urllib.error
import urllib.parse
import urllib.request

SPEC=importlib.util.spec_from_file_location('native_todoist_desktop_tests',Path(__file__).with_name('todoist-desktop.py'))
desktop=importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name]=desktop
SPEC.loader.exec_module(desktop)


class CaptureFraming(unittest.TestCase):
    def test_project_capture_reveals_the_undated_details_before_inspecting_them(self):
        with tempfile.TemporaryDirectory(prefix='buddy-undated-capture-') as temporary:
            root = Path(temporary)
            config = root / 'config.json'
            config.write_text('{"native":{}}')
            api = Mock()
            api.snapshot.return_value = dict(tasks=[], projects=[], created=0, traffic=[])
            driver = Mock()
            stopped = RuntimeError('stopped before any native input')
            driver.drive.side_effect = stopped
            with patch.object(desktop.github, 'installed_binary', return_value={}), \
                    patch.object(desktop, 'fixture', return_value=config), \
                    patch.object(desktop.settings, 'Driver', return_value=driver), \
                    patch.object(desktop.ScenarioFixture, 'start', return_value=api):
                with self.assertRaises(RuntimeError) as failure:
                    desktop.qualify(root / 'unused.exe', root, 'windows', 'test', Mock(url='http://127.0.0.1:9'))
            self.assertIs(failure.exception, stopped)
            actions = driver.drive.call_args.args[1]
            capture = actions.index(('capture', 'project-undated-dark'))
            self.assertEqual(actions[capture - 1], ('wait_scroll_text', 'No due date', 'Synthetic undated task'))
            api.close.assert_called_once()

    def test_calendar_day_capture_collapses_and_restores_overdue_before_leaving_today(self):
        with tempfile.TemporaryDirectory(prefix='buddy-calendar-capture-') as temporary:
            root = Path(temporary)
            config = root / 'config.json'
            config.write_text('{"native":{}}')
            api = Mock()
            api.snapshot.return_value = dict(tasks=[], projects=[], created=0, traffic=[])
            driver = Mock()
            driver.drive.side_effect = RuntimeError('stopped before any native input')
            with patch.object(desktop.github, 'installed_binary', return_value={}), \
                    patch.object(desktop, 'fixture', return_value=config), \
                    patch.object(desktop.settings, 'Driver', return_value=driver), \
                    patch.object(desktop.ScenarioFixture, 'start', return_value=api):
                with self.assertRaises(RuntimeError):
                    desktop.qualify(root / 'unused.exe', root, 'windows', 'test', Mock(url='http://127.0.0.1:9'))
            actions = driver.drive.call_args.args[1]
            today = actions.index(('capture', 'today-dark'))
            self.assertEqual(actions[today + 1:today + 8], [
                ('content', 'Collapse Overdue'), ('wait_text', 'Expand Overdue'),
                ('capture', 'today-overdue-collapsed-dark'),
                ('content', 'Expand Overdue'), ('wait_text', 'Synthetic overdue task'),
                ('capture', 'today-overdue-expanded-dark'), ('content', 'Upcoming')])


class ElectronCarryOver(unittest.TestCase):
    def test_each_import_run_must_print_its_own_static_outcome_and_no_value(self):
        imported = desktop.CARRY_IMPORTED + ' No service requests were sent.'
        kept = desktop.CARRY_KEPT + ' No credentials were changed.'
        desktop.check_carry_outputs(imported, kept)
        for first, second in ((kept, kept), (imported, imported), ('', kept), (imported, ''),
                (imported + ' ' + desktop.todoist.TOKEN, kept),
                (imported, kept + ' ' + desktop.UNREADABLE_REPEAT_TOKEN)):
            with self.subTest(first=first, second=second):
                with self.assertRaisesRegex(RuntimeError, 'expected static outcome'):
                    desktop.check_carry_outputs(first, second)

    def test_the_carry_over_seeds_an_empty_vault_once_then_restores_it(self):
        source = Path(desktop.__file__).read_text(encoding='utf-8')
        # The native token validator refuses whitespace, so only a repeat that
        # never reads the environment can succeed with this value.
        self.assertRegex(desktop.UNREADABLE_REPEAT_TOKEN, r'\s')
        self.assertNotEqual(desktop.UNREADABLE_REPEAT_TOKEN, desktop.todoist.TOKEN)
        order = [source.index(marker) for marker in (
            "driver.drive('forgotten-restart-light'",
            "('wait_text', 'Synthetic today task'), ('capture', 'todoist-credential-carried-dark')",
            '\n            carry_over(executable, driver, api, config, dark, check_carried)',
        )]
        self.assertEqual(order, sorted(order))
        self.assertEqual(source.count("'--import-electron-todoist'"), 2)
        self.assertIn("absent=('No API token is saved',)", source[order[0]:order[-1]])

    def test_the_receipt_claims_only_what_the_scene_checks_and_names_the_no_reread_test(self):
        source = Path(desktop.__file__).read_text(encoding='utf-8')
        self.assertNotIn('carriedTokenKeptWithoutRereading', source)
        self.assertIn('repeatImportKeptOutcomeAndCredential=True', source)
        test = 'an_existing_native_token_is_kept_and_the_source_is_never_read'
        self.assertIn(test, source)
        rust = (Path(__file__).resolve().parents[2] / 'rust/crates/buddy-core/src/todoist/credentials'
            / 'electron_import/tests.rs').read_text(encoding='utf-8')
        body = rust[rust.index('fn ' + test):]
        body = body[:body.index('\n}\n')]
        # The source callback panics, so the kept outcome proves no read.
        self.assertIn('panic!', body)

    def probing_driver(self, api, reach=True, drives=()):
        """A driver whose no-capture probes reach the fixture when `reach`
        says so (a bool, or the set of probe names that reach it). Every other
        drive takes the next of `drives`: None succeeds, an exception fails."""
        others = list(drives)
        self.events = []

        def drive(name, actions):
            self.events.append(name)
            if actions == [('sleep', desktop.PROBE_SECONDS)]:
                if reach is True or (reach is not False and name in reach):
                    api.traffic.append(dict(method='GET', path='/api/v1/user'))
                return None
            result = others.pop(0) if others else None
            if isinstance(result, BaseException):
                raise result
            return None
        driver = Mock()
        driver.drive.side_effect = drive
        return driver

    def carry(self, runs, drives=(), check=None, reach=True, machine=False):
        """Run carry_over against mocks. Returns (driver, environments, error)."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        config = Path(directory.name) / 'config.json'
        config.write_text(json.dumps(dict(ui=dict(theme='light'))))
        environments = []

        def run(executable, argument):
            self.assertEqual(json.loads(config.read_text())['ui'], dict(theme='dark'))
            environments.append(desktop.os.environ.get('TODOIST_API_TOKEN'))
            result = runs.pop(0)
            if isinstance(result, BaseException):
                raise result
            return result
        api = Mock(traffic=[])
        api.snapshot.side_effect = lambda: dict(traffic=list(api.traffic))
        driver = self.probing_driver(api, reach, drives)
        error = None
        with patch.object(desktop.settings, 'run', side_effect=run), \
                patch.object(desktop, 'machine_token_present', return_value=machine):
            try:
                desktop.carry_over('buddy', driver, api, config, dict(theme='dark'), check or Mock())
            except BaseException as raised:
                error = raised
        self.assertEqual(json.loads(config.read_text())['ui'], dict(theme='light'))
        return driver, environments, error

    @staticmethod
    def names(driver):
        return [call.args[0] for call in driver.drive.call_args_list]

    def test_a_confirmed_carry_over_reaches_the_fixture_before_capturing_then_forgets(self):
        check = Mock(side_effect=lambda: self.events.append('check'))
        driver, environments, error = self.carry(
            [desktop.CARRY_IMPORTED, desktop.CARRY_KEPT], check=check)
        self.assertIsNone(error)
        self.assertEqual(environments, [desktop.todoist.TOKEN, desktop.UNREADABLE_REPEAT_TOKEN])
        check.assert_called_once_with()
        # The probe ran, with no capture, before the Tasks view was captured.
        self.assertEqual(self.events, ['credential-probe-dark', 'check', 'carried-token-forget-light'])
        self.assertEqual(driver.drive.call_args_list[0].args[1], [('sleep', desktop.PROBE_SECONDS)])
        self.assertEqual(self.names(driver), ['credential-probe-dark', 'carried-token-forget-light'])
        self.assertEqual(driver.drive.call_args.args[1][-2:], list(desktop.FORGET_STEPS))

    def test_a_carried_value_that_never_reaches_the_fixture_is_never_captured_or_opened(self):
        # Any value but the synthetic token uses the production client, so
        # the fixture sees nothing: the run fails before the Tasks capture,
        # and the cleanup does not open the UI either.
        check = Mock()
        driver, _, error = self.carry([desktop.CARRY_IMPORTED, desktop.CARRY_KEPT], check=check, reach=False)
        self.assertEqual(str(error), desktop.CARRIED_UNVERIFIED)
        check.assert_not_called()
        self.assertEqual(self.names(driver), ['credential-probe-dark', 'forget-probe-light'])
        for call in driver.drive.call_args_list:
            self.assertEqual(call.args[1], [('sleep', desktop.PROBE_SECONDS)])
        self.assertTrue(any('Forgetting the carried Todoist token also failed' in note
            for note in getattr(error, '__notes__', [])))

    def test_a_machine_scope_token_refuses_the_carry_over_before_anything_runs(self):
        check = Mock()
        driver, environments, error = self.carry([], check=check, machine=True)
        self.assertEqual(str(error), desktop.MACHINE_TOKEN_REFUSED)
        self.assertEqual(environments, [])
        driver.drive.assert_not_called()
        check.assert_not_called()

    def test_machine_scope_check_reads_only_on_windows_and_ignores_only_blank_text(self):
        never = Mock(side_effect=AssertionError('read outside Windows'))
        for platform in ('linux', 'darwin'):
            self.assertFalse(desktop.machine_token_present(platform, never))
        never.assert_not_called()
        for value, present in ((None, False), ('', False), (' \r\n', False), ('value', True),
                (' value ', True), ('%VALUE%', True), (0, True), (b'', True)):
            with self.subTest(value=value):
                self.assertIs(desktop.machine_token_present('win32', lambda: value), present)
        denied = Mock(side_effect=PermissionError('denied'))
        with self.assertRaises(RuntimeError) as failure:
            desktop.machine_token_present('win32', denied)
        self.assertEqual(str(failure.exception), desktop.MACHINE_UNREADABLE)
        self.assertIsNone(failure.exception.__cause__)

    def test_machine_scope_read_uses_the_environment_key_and_treats_a_missing_name_as_none(self):
        opened = []

        class Key:
            def __enter__(self):
                return self

            def __exit__(self, *exception):
                return False
        values = {}

        def query(key, name):
            opened.append(name)
            if name not in values:
                raise FileNotFoundError(name)
            return values[name], 1
        winreg = Mock(HKEY_LOCAL_MACHINE='HKLM')
        winreg.OpenKey.side_effect = lambda root, path: opened.append((root, path)) or Key()
        winreg.QueryValueEx.side_effect = query
        with patch.dict(sys.modules, winreg=winreg):
            self.assertIsNone(desktop.read_machine_token())
            values['TODOIST_API_TOKEN'] = 'synthetic machine value'
            self.assertEqual(desktop.read_machine_token(), 'synthetic machine value')
            winreg.OpenKey.side_effect = FileNotFoundError('no key')
            self.assertIsNone(desktop.read_machine_token())
        self.assertEqual(opened[:2], [('HKLM', desktop.MACHINE_ENVIRONMENT), 'TODOIST_API_TOKEN'])
        self.assertEqual(desktop.MACHINE_ENVIRONMENT,
            'SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment')

    def test_a_confirmed_import_whose_fixture_connection_fails_forgets_on_the_form(self):
        # The probe reached the fixture, but the connection then failed, so
        # the launch shows the connection form and no synthetic task.
        failed = RuntimeError('no synthetic task')
        driver, _, error = self.carry([desktop.CARRY_IMPORTED, desktop.CARRY_KEPT],
            check=Mock(side_effect=failed), drives=[RuntimeError('no tasks'), None])
        self.assertIs(error, failed)
        self.assertEqual(self.names(driver),
            ['credential-probe-dark', 'carried-token-forget-light', 'carried-token-forget-form-light'])
        self.assertEqual(driver.drive.call_args.args[1], list(desktop.FORGET_STEPS))
        self.assertFalse(getattr(error, '__notes__', []))

    def test_a_failed_read_back_after_the_write_still_forgets_the_token(self):
        # Exit 1 after the vault write: settings.run raises before the import
        # is confirmed, yet the synthetic token may be stored.
        failed = subprocess.CalledProcessError(1, 'buddy')
        driver, environments, error = self.carry([failed])
        self.assertIs(error, failed)
        self.assertEqual(environments, [desktop.todoist.TOKEN])
        self.assertEqual(self.names(driver), ['forget-probe-light', 'carried-token-forget-light'])
        # The connection form is tried when the launch shows no tasks.
        driver, _, error = self.carry([RuntimeError('import exited 1')], drives=[RuntimeError('no tasks'), None])
        self.assertIsInstance(error, RuntimeError)
        self.assertEqual(str(error), 'import exited 1')
        self.assertEqual(self.names(driver),
            ['forget-probe-light', 'carried-token-forget-light', 'carried-token-forget-form-light'])
        self.assertEqual(driver.drive.call_args.args[1], list(desktop.FORGET_STEPS))
        # A vault the launch cannot connect with is not opened in the UI.
        driver, _, error = self.carry([RuntimeError('import exited 1')], reach=False)
        self.assertEqual(str(error), 'import exited 1')
        self.assertEqual(self.names(driver), ['forget-probe-light'])

    def test_a_failed_forget_cannot_hide_the_import_failure(self):
        driver, _, error = self.carry([RuntimeError('import exited 1')],
            drives=[RuntimeError('no tasks'), RuntimeError('no form')])
        self.assertEqual(str(error), 'import exited 1')
        self.assertEqual(driver.drive.call_count, 3)
        # Without an earlier failure, a failed forget fails the run.
        driver, _, error = self.carry([desktop.CARRY_IMPORTED, desktop.CARRY_KEPT],
            drives=[RuntimeError('forget failed'), RuntimeError('form failed')])
        self.assertEqual(str(error), 'form failed')

    def carry_with_failed_restore(self, runs, drives=()):
        """Run carry_over where restoring the light theme raises."""
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        config = Path(directory.name) / 'config.json'
        config.write_text(json.dumps(dict(ui=dict(theme='light'))))
        restore = PermissionError('config is read-only')
        real = desktop.set_ui

        def set_ui(path, ui):
            if ui == dict(theme='light'):
                raise restore
            real(path, ui)
        api = Mock(traffic=[])
        api.snapshot.side_effect = lambda: dict(traffic=list(api.traffic))
        driver = self.probing_driver(api, True, drives)
        error = None
        with patch.object(desktop.settings, 'run', side_effect=list(runs)), \
                patch.object(desktop, 'set_ui', side_effect=set_ui), \
                patch.object(desktop, 'machine_token_present', return_value=False):
            try:
                desktop.carry_over('buddy', driver, api, config, dict(theme='dark'), Mock())
            except BaseException as raised:
                error = raised
        return driver, restore, error

    def test_a_failed_theme_restore_still_forgets_the_token(self):
        driver, restore, error = self.carry_with_failed_restore([desktop.CARRY_IMPORTED, desktop.CARRY_KEPT])
        # The Forget ran, then the restore failure fails the run.
        self.assertEqual(self.names(driver), ['credential-probe-dark', 'carried-token-forget-light'])
        self.assertIs(error, restore)
        # A failed Forget too is reported on the restore failure, not instead of it.
        driver, restore, error = self.carry_with_failed_restore(
            [desktop.CARRY_IMPORTED, desktop.CARRY_KEPT],
            drives=[RuntimeError('forget failed'), RuntimeError('form failed')])
        self.assertIs(error, restore)
        self.assertEqual(driver.drive.call_count, 3)
        self.assertTrue(any('Forgetting the carried Todoist token also failed' in note
            for note in getattr(error, '__notes__', [])))

    def test_a_failed_theme_restore_cannot_replace_the_import_failure(self):
        failed = RuntimeError('import exited 1')
        driver, _, error = self.carry_with_failed_restore([failed], drives=[None])
        self.assertIs(error, failed)
        self.assertEqual(self.names(driver), ['forget-probe-light', 'carried-token-forget-light'])
        self.assertTrue(any('Restoring the fixture theme also failed' in note
            for note in getattr(error, '__notes__', [])))

    def test_a_failure_before_any_import_never_drives_a_forget(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.json'
            config.write_text('not json')
            driver = Mock()
            with patch.object(desktop, 'machine_token_present', return_value=False), \
                    self.assertRaises(ValueError):
                desktop.carry_over('buddy', driver, Mock(), config, dict(theme='dark'), Mock())
            driver.drive.assert_not_called()

    def test_set_ui_replaces_only_the_theme_section(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.json'
            config.write_text(json.dumps(dict(ui=dict(theme='light', zoom=1), native=dict(activeSection='tasks'))))
            desktop.set_ui(config, dict(theme='dark'))
            self.assertEqual(json.loads(config.read_text()), dict(ui=dict(theme='dark'), native=dict(activeSection='tasks')))


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
