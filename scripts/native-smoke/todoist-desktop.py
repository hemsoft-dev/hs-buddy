"""Actual installed Todoist UI, real OS vault and owned loopback service only."""
import argparse
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import struct
import sys

from desktop import fixture

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location('todoist_desktop_' + name.replace('-', '_'), HERE / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


todoist = load('todoist-fixture')
github = load('github-desktop')
settings = github.settings


class ScenarioFixture(todoist.Fixture):
    """Arm the next failure only after the app completed a real full read."""
    def __init__(self, marker):
        self.fail_after_read = None
        self.complete_tasks = False
        super().__init__(marker)

    def arm(self, scenario):
        if scenario not in ('offline', 'expired', 'task-read'):
            raise ValueError('Unsupported Todoist failure scenario')
        with self.lock:
            self.complete_tasks = False
            self.fail_after_read = scenario

    def _get(self, route, query):
        value = super()._get(route, query)
        if route == 'tasks' and value['next_cursor'] is None:
            self.complete_tasks = True
        elif route == 'user' and self.complete_tasks and self.fail_after_read:
            if self.fail_after_read == 'offline':
                self.disconnect = True
            elif self.fail_after_read == 'task-read':
                self.fail_next_tasks = True
            else:
                self.expired = True
            self.fail_after_read = None
            self.complete_tasks = False
        return value

    def add_permission_projects(self):
        with self.lock:
            for index, (identifier, name, role) in enumerate([
                    ('project-complete', 'Synthetic Complete Only', 'COMPLETE_ONLY'),
                    ('project-readonly', 'Synthetic Read Only', 'FUTURE_ROLE')]):
                self.projects.append(dict(id=identifier, name=name, color='blue', parent_id=None,
                    child_order=index+3, workspace_id='synthetic-team', role=role))
                self.tasks.append(todoist._task('task-'+identifier, name+' task', identifier))

    def add_large_account(self):
        with self.lock:
            self.page_cap = 100
            for index in range(17):
                self.projects.append(dict(id=f'project-bulk-{index}', name=f'Synthetic Bulk {index}',
                    color='blue', parent_id=None, child_order=index+5))
            self.tasks.extend(todoist._task(f'bulk-task-{index}', f'Synthetic bulk task {index}',
                f'project-bulk-{index//300}') for index in range(5001))


@contextmanager
def environment(changes):
    previous = {key: os.environ.get(key) for key in changes}
    try:
        os.environ.update({key: str(value) for key, value in changes.items()})
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def posts(api):
    return [row for row in api.snapshot()['traffic'] if row['method'] == 'POST']


def pending_bytes(config):
    paths = list(config.parent.glob('native-todoist-change-*.json'))
    if len(paths) != 1 or paths[0].is_symlink():
        raise RuntimeError('Expected exactly one original native Todoist recovery record')
    return paths[0].read_bytes()


def check_task(api, identifier, **expected):
    task = next(row for row in api.snapshot()['tasks'] if row['id'] == identifier)
    for key, value in expected.items():
        if task[key] != value:
            raise RuntimeError(f'Todoist UI did not apply the expected synthetic {key}')


def captures(output):
    result = []
    for path in sorted(output.glob('*/*.png')):
        if path.stem in ('locator', 'wait-status', 'failed-click') or '.ocr-analysis-' in path.name:
            continue
        data = path.read_bytes()
        if data[:8] != b'\x89PNG\r\n\x1a\n':
            raise RuntimeError('Invalid Todoist capture')
        width, height = struct.unpack('>II', data[16:24])
        result.append(dict(path=path.relative_to(output).as_posix(), width=width, height=height,
                           sha256=hashlib.sha256(data).hexdigest()))
    if not result:
        raise RuntimeError('Missing installed Todoist captures')
    return result


def qualify(executable, output, session, version, backend):
    binary = github.installed_binary(executable, version)
    config = fixture(output, 'dark')
    value = json.loads(config.read_text())
    value['native']['activeSection'] = 'tasks'
    config.write_text(json.dumps(value), encoding='utf-8')
    driver = settings.Driver(executable, output, session, backend, HERE.parent.parent)
    with environment(dict(BUDDY_TODOIST_FIXTURE='1', BUDDY_CONFIG_PATH=config,
            BUDDY_CONVEX_URL=backend.url, RUST_LOG='warn,reqwest=trace,hyper=trace')):
        api = ScenarioFixture.start(config)
        api.add_permission_projects()
        primary = None
        try:
            initial = api.snapshot()
            (output / 'initial-tasks.json').write_text(json.dumps(initial, indent=2), encoding='utf-8')
            connected = driver.drive('connect-dark', [
                ('wait_text', 'No API token is saved'), ('capture', 'missing-token-dark'),
                ('input', 'Todoist API token', todoist.TOKEN), ('click', 'Save and connect'),
                ('wait_text', 'Synthetic today task'), ('capture', 'today-dark'),
                ('click', 'Upcoming'), ('wait_text', 'Synthetic upcoming task'), ('capture', 'upcoming-dark'),
                ('click', 'Projects'), ('click', 'Synthetic Beta'), ('wait_text', 'Synthetic undated task'),
                ('capture', 'project-undated-dark'), ('resize', 900, 650), ('key', 'Tab'),
                ('capture', 'projects-compact-keyboard-dark'),
            ])
            github.check(connected / 'today-dark.png', 'America/Winnipeg', 'Synthetic today task', 'Overdue')
            github.check(connected / 'project-undated-dark.png', 'Synthetic undated task', 'No due date')
            if posts(api):
                raise RuntimeError('Read-only Todoist navigation sent a mutation')

            api.drop_next_ack = True
            lost = driver.drive('lost-ack-dark', [
                ('wait_text', 'Synthetic today task'), ('click', 'New task'),
                ('input', 'Task name', 'Synthetic created task 界'),
                ('wait_scroll_text', 'Save task'), ('click', 'Save task'),
                ('wait_text', 'The change is unconfirmed'), ('capture', 'lost-ack-dark'),
            ])
            github.check(lost / 'lost-ack-dark.png', 'unconfirmed', 'Retry saved change')
            original = pending_bytes(config)
            (output / 'original-recovery.json').write_bytes(original)
            if len(posts(api)) != 1 or api.snapshot()['created'] != 1:
                raise RuntimeError('The unconfirmed task was not sent exactly once')
            check_task(api, 'task-created-1', content='Synthetic created task 界')

            light = json.loads(fixture(output / 'light-template', 'light').read_text())['ui']
            value = json.loads(config.read_text())
            value['ui'].update(light)
            config.write_text(json.dumps(value), encoding='utf-8')
            # First reopen only. A separate process proves recovery never resends.
            recovered = driver.drive('recovered-light', [
                ('wait_text', 'Retry saved change'), ('capture', 'recovered-light'),
                ('click', 'Discard saved recovery'),
                ('wait_scroll_above', 'Keep recovery', 'Filter tasks'), ('capture', 'discard-confirmation-light'),
                ('click', 'Keep recovery'),
                ('click', 'Refresh'), ('wait_text', 'Tasks refreshed'),
            ])
            if pending_bytes(config) != original or len(posts(api)) != 1:
                raise RuntimeError('Restart or refresh changed/resubmitted the recovery record')
            retried = driver.drive('retry-light', [
                ('wait_text', 'Retry saved change'), ('click', 'Retry saved change'),
                ('wait_text', 'Todoist confirmed the change'), ('capture', 'confirmed-retry-light'),
                ('click', 'Projects'), ('click', 'Synthetic Alpha'), ('wait_text', 'Synthetic created task'),
                ('capture', 'created-task-light'),
                ('click', 'Synthetic Empty'), ('wait_text', 'No tasks in this view'),
                ('capture', 'empty-project-light'),
            ])
            requests = [row for row in api.snapshot()['requests'] if row['method'] == 'POST']
            if len(posts(api)) != 2 or len(requests) != 2 or requests[0]['commands'] != requests[1]['commands'] or api.snapshot()['created'] != 1:
                raise RuntimeError('Explicit retry did not preserve one immutable creation command')
            if list(config.parent.glob('native-todoist-change-*.json')):
                raise RuntimeError('Acknowledged retry left a recovery record')

            # Narrow the real list before operating its row, so identical action
            # labels cannot accidentally target a different synthetic task.
            edited = driver.drive('edit-light', [
                ('wait_text', 'Synthetic today task'), ('click', 'Projects'), ('click', 'Synthetic Beta'),
                ('input', 'Filter tasks', 'Synthetic undated task'), ('wait_text', 'Synthetic undated task'),
                ('click', 'Edit'), ('input', 'Task name', 'Synthetic undated task revised'),
                ('wait_scroll_text', 'Save task'), ('click', 'Save task'),
                ('wait_text', 'Todoist confirmed the change'), ('capture', 'edited-light'),
                ('click', 'Complete'), ('wait_text', 'Todoist confirmed the change'), ('capture', 'completed-light'),
                ('input', 'Filter tasks', 'Synthetic recurring task'), ('wait_text', 'Synthetic recurring task'),
                ('click', 'Complete'), ('wait_text', 'Todoist confirmed the change'),
                ('capture', 'recurring-completed-light'),
                ('click', 'Delete'), ('wait_text', 'Confirm deletion'), ('click', 'Confirm deletion'),
                ('wait_text', 'Todoist confirmed the change'), ('capture', 'deleted-light'),
            ], record=True)
            check_task(api, 'task-undated', content='Synthetic undated task revised', checked=True)
            check_task(api, 'task-recurring', is_deleted=True, due=todoist._due('2026-11-03', recurring=True))
            if len(posts(api)) != 6:
                raise RuntimeError('Editing, completing or deleting sent an unexpected task write')

            permissions = driver.drive('project-permissions-light', [
                ('wait_text', 'Synthetic today task'), ('click', 'Projects'), ('click', 'Synthetic Complete Only'),
                ('wait_text', 'This project permits completing tasks only'), ('capture', 'complete-only-light'),
                ('click', 'Complete'), ('wait_text', 'Todoist confirmed the change'),
                ('wait_text', 'No tasks in this view'), ('capture', 'complete-only-completed-light'),
                ('click', 'Synthetic Read Only'), ('wait_text', 'Tasks are read-only'),
                ('capture', 'read-only-project-light'),
            ])
            github.check(permissions / 'complete-only-light.png', 'completing tasks only', 'Synthetic Complete Only task')
            github.check(permissions / 'read-only-project-light.png', 'read-only', 'Synthetic Read Only task')
            check_task(api, 'task-project-complete', checked=True)
            if len(posts(api)) != 7:
                raise RuntimeError('Project permission navigation sent an unexpected task write')

            api.reject_next_auth = True
            driver.drive('connection-recheck-light', [
                ('wait_text', 'Todoist rejected this connection'), ('capture', 'initial-auth-error-light'),
                ('click', 'Recheck saved connection'), ('wait_text', 'Synthetic today task'),
                ('capture', 'saved-connection-rechecked-light'),
            ])
            if len(posts(api)) != 7:
                raise RuntimeError('Rechecking a failed saved connection sent a task mutation')

            for scenario, problem in [('offline', 'Could not reach Todoist'), ('expired', 'Todoist rejected this connection')]:
                api.arm(scenario)
                phase = driver.drive(scenario + '-light', [
                    ('wait_text', 'Synthetic today task'), ('click', 'Refresh'), ('wait_text', problem),
                    ('capture', scenario + '-retained-light'), ('resize', 900, 650),
                    ('capture', scenario + '-compact-light'),
                    ('click', 'Connection'), ('click', 'Recheck saved connection'),
                    ('wait_text', problem), ('capture', scenario + '-reconnect-retained-light'),
                ])
                github.check(phase / (scenario + '-retained-light.png'), problem, 'Synthetic today task')
                github.check(phase / (scenario + '-reconnect-retained-light.png'), problem, 'Synthetic today task', 'synthetic-todoist-account')
                if len(posts(api)) != 7:
                    raise RuntimeError('An unsuccessful read sent another task mutation')
                with api.lock:
                    api.disconnect = False
                    api.expired = False

            api.arm('task-read')
            retained = driver.drive('failed-replacement-read-light', [
                ('wait_text', 'Synthetic today task'), ('click', 'Connection'), ('click', 'Recheck saved connection'),
                ('wait_text', 'Todoist could not finish this request'), ('capture', 'replacement-read-retained-light'),
            ])
            github.check(retained / 'replacement-read-retained-light.png', 'could not finish', 'Synthetic today task', 'synthetic-todoist-account')
            if len(posts(api)) != 7:
                raise RuntimeError('A failed replacement read sent a task write')

            api.add_large_account()
            scoped = driver.drive('large-account-scopes-light', [
                ('wait_text', 'Choose a smaller scope'), ('capture', 'large-account-limit-light'),
                ('click', 'Load calendar tasks'), ('wait_text', 'Task scope loaded'),
                ('wait_text', 'Synthetic upcoming task'), ('capture', 'calendar-scope-light'),
                ('click', 'Projects'), ('click', 'Synthetic Alpha'), ('wait_text', 'Synthetic created task'),
                ('capture', 'project-scope-undated-light'),
            ])
            github.check(scoped / 'large-account-limit-light.png', 'smaller scope', 'Load calendar tasks')
            github.check(scoped / 'project-scope-undated-light.png', 'Synthetic created task', 'No due date')
            if len(posts(api)) != 7:
                raise RuntimeError('Loading smaller task scopes sent a mutation')

            before_forget = api.snapshot()['tasks']
            driver.drive('forget-token-light', [
                ('wait_text', 'Choose a smaller scope'), ('click', 'Connection'),
                ('click', 'Forget saved token'), ('wait_text', 'The saved Todoist token was removed'),
                ('capture', 'token-removed-light'),
            ])
            driver.drive('forgotten-restart-light', [
                ('wait_text', 'No API token is saved'), ('capture', 'missing-after-restart-light'),
            ])
            if api.snapshot()['tasks'] != before_forget or len(posts(api)) != 7:
                raise RuntimeError('Removing the local credential changed service tasks')

            for path in output.glob('*/app.log'):
                text = path.read_text(encoding='utf-8', errors='replace')
                if any(private in text for private in (todoist.TOKEN, 'Synthetic created task', 'Synthetic undated task')):
                    raise RuntimeError('Private Todoist content appeared in app tracing')
            final = api.snapshot()
            (output / 'requests.json').write_text(json.dumps(final, indent=2), encoding='utf-8')
            receipt = dict(result='passed', version=version, session=session, **binary,
                largeWindow=driver.large_size, compactWindow=[900, 650], screenshots=captures(output),
                readOnlyNavigationPosts=0, taskWriteAttempts=7, createdTasks=1,
                restrictedProjectCompletionVerified=True, unknownProjectRoleReadOnly=True,
                failedReconnectRetainedAccountTasks=True, failedReplacementReadRetainedAccountTasks=True,
                largeAccountScopedReadsVerified=True,
                recurringCompletionAdvancedExactlyOneDay=True,
                originalRecoverySha256=hashlib.sha256(original).hexdigest(), sameCommandRetried=True,
                realOsCredentialStore=True, secureTokenSurvivedProcessRestart=True,
                secureTokenRemovalVerifiedAfterRestart=True,
                initialConnectionErrorRecheckedWithoutTokenEntry=True,
                expiredAndOfflineReadRetainedTasks=True, privateTraceContentAbsent=True,
                dataSource='Owned numeric loopback Todoist fixture and isolated anonymous Convex')
        except BaseException as failure:
            primary = failure
            try:
                (output / 'failed-requests.json').write_text(json.dumps(api.snapshot(), indent=2), encoding='utf-8')
            except BaseException as diagnostic:
                failure.add_note('Todoist request diagnostics failed: ' + type(diagnostic).__name__)
            raise
        finally:
            from desktop import finish_cleanup
            finish_cleanup(api.close, primary, 'Todoist fixture shutdown')
        (output / 'qualification.json').write_text(json.dumps(receipt, indent=2), encoding='utf-8')


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('BUDDY_SETTINGS_FIXTURE') != '1':
        raise RuntimeError('Todoist qualification requires an owned disposable GitHub Actions desktop')
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--session', choices=('windows', 'macos', 'x11'), required=True)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    terminal = load('terminal-desktop')
    output = terminal.owned_output(args.output)
    with settings.SettingsBackend(HERE.parent.parent) as backend:
        qualify(args.executable.resolve(strict=True), output, args.session, args.version, backend)


if __name__ == '__main__':
    main()
