"""Guard native menu captures and reject incorrect platform accelerators."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('macos_menus', Path(__file__).with_name('macos-menus.py'))
menus = importlib.util.module_from_spec(spec)
spec.loader.exec_module(menus)
desktop = sys.modules[menus.UnixWindow.__module__]

PID = 76900
# Retained Intel failure, job 127361569902 of run 42060768660.
HOSTED_861 = ('osascript failed with exit 1: 104:110: execution error: System Events got an error: '
    'Can’t get menu bar 1 of application process 1 whose unix id = 76900. Invalid index. (-1719)')
GOOD_ROWS = '\n'.join(label + '\t' + (key[0].upper() if key else '') + '\t'
    + str(key[1] if key else -1) + '\ttrue\t-1\t-1'
    for label, key in menus.CONTRACT['Buddy Native'].items())


def owned_window(*polls):
    """The real owned-PID window driver over a fake Popen. Each poll() takes
    the next answer, and the last one repeats."""
    answers = list(polls)
    def poll():
        answer = answers.pop(0) if len(answers) > 1 else answers[0]
        if isinstance(answer, BaseException):
            raise answer
        return answer
    process = Mock()
    process.pid = PID
    process.poll.side_effect = poll
    return menus.UnixWindow(process, 'macos')


class OwnedDesktop:
    """Fake process boundary for every command capture() and Escape send."""
    def __init__(self, **responses):
        self.responses = dict(dict(bounds='100, 25, 300, 400', rows=GOOD_ROWS), **responses)
        self.calls = []

    @staticmethod
    def kind(args):
        if args[0] == 'screencapture':
            return 'screencapture'
        script = str(args[-1])
        for marker, name in (('click menu bar item', 'click'), ('set p to position of popup', 'bounds'),
                ('set menuRows', 'rows'), ('set barCount', 'observe'), ('key code 53', 'escape')):
            if marker in script:
                return name
        return 'other'

    def __call__(self, *args, **kwargs):
        name = self.kind(args)
        self.calls.append((name, [str(arg) for arg in args], kwargs))
        response = self.responses.get(name, '')
        if isinstance(response, BaseException):
            raise response
        if name == 'screencapture':
            Path(args[-1]).write_bytes(response or b'\x89PNG\r\n\x1a\n')
            return ''
        return response

    def names(self):
        return [name for name, _, _ in self.calls]


def failure_snapshot(destination):
    return menus.json.loads(destination.with_name(destination.stem + '.failure.json').read_text())


def fields(frontmost=None, bars=None, windows=None):
    def field(value):
        return dict(state='unavailable') if value is None else dict(state='available', value=value)
    return dict(frontmost=field(frontmost), menuBarCount=field(bars), windowCount=field(windows))


class MacMenus(unittest.TestCase):
    def test_capture_never_runs_on_a_developer_desktop_or_outside_the_fixture(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(menus.platform, 'system', return_value='Darwin'):
            root = Path(temporary).resolve()
            with patch.dict(os.environ, dict(GITHUB_ACTIONS='true', BUDDY_SETTINGS_FIXTURE='1', RUNNER_TEMP=str(root))):
                menus.guard(root / 'menus')
                for bad in (root, root.parent / 'other'):
                    with self.assertRaises(RuntimeError):
                        menus.guard(bad)
            with patch.dict(os.environ, dict(GITHUB_ACTIONS='false')):
                with self.assertRaises(RuntimeError):
                    menus.guard(root / 'menus')

    def test_native_accelerators_reject_missing_command_wrong_modifiers_and_duplicates(self):
        rows = [dict(label=label, key=key[0] if key else '', modifiers=key[1] if key else -1)
            for label, key in menus.CONTRACT['Buddy Native'].items()]
        menus.verify('Buddy Native', rows)
        for change in (dict(key='x'), dict(modifiers=8), dict(modifiers=1)):
            bad = [dict(row) for row in rows]
            bad[1].update(change)
            with self.assertRaises(RuntimeError):
                menus.verify('Buddy Native', bad)
        with self.assertRaises(RuntimeError):
            menus.verify('Buddy Native', rows + [rows[1]])
        with self.assertRaises(RuntimeError):
            menus.verify('Buddy Native', rows[1:])

    def test_ax_uppercase_letters_do_not_imply_shift(self):
        for menu in menus.CONTRACT:
            rows = [dict(label=label, key=key[0].upper() if key else '',
                modifiers=key[1] if key else -1, enabled=True)
                for label, key in menus.CONTRACT[menu].items()]
            menus.verify(menu, rows)
            for label, wrong_mask in (('Hide Buddy Native', 1), ('Hide Buddy Native', 8),
                    ('Hide Others', 0), ('Redo', 0), ('Undo', 1)):
                if label in menus.CONTRACT[menu]:
                    bad = [dict(row, modifiers=wrong_mask) if row['label'] == label else row for row in rows]
                    with self.assertRaises(RuntimeError):
                        menus.verify(menu, bad)

    def test_mismatched_menu_retains_pixels_and_raw_attributes(self):
        window = Mock()
        window.process.poll.return_value = None
        window.apple.side_effect = lambda script: script
        rows = '\n'.join(label + '\t' + (key[0].upper() if key else '') + '\t'
            + str(8 if label == 'Hide Buddy Native' else key[1] if key else -1) + '\ttrue\t-1\t-1'
            for label, key in menus.CONTRACT['Buddy Native'].items())
        def run(*args):
            if args[0] == 'screencapture':
                Path(args[-1]).write_bytes(b'\x89PNG\r\n\x1a\n')
                return ''
            if 'set p to position' in args[-1]:
                return '100, 25, 300, 400'
            return rows if 'set menuRows' in args[-1] else ''
        with tempfile.TemporaryDirectory() as directory, patch.object(menus, 'run', side_effect=run), \
                patch.object(menus.time, 'sleep'):
            image = Path(directory) / 'app.png'
            with self.assertRaises(RuntimeError):
                menus.capture(window, 'Buddy Native', image)
            self.assertTrue(image.is_file())
            metadata = menus.json.loads(image.with_suffix('.json').read_text())
            hide = next(item for item in metadata['items'] if item['label'] == 'Hide Buddy Native')
            self.assertEqual((hide['key'], hide['modifiers']), ('H', 8))

    def test_unbound_actions_reject_character_modifier_and_special_key_accelerators(self):
        for menu, contract in menus.CONTRACT.items():
            rows = [dict(label=label, key=key[0].upper() if key else '',
                modifiers=key[1] if key else 0, enabled=True, glyph=None if key else 0)
                for label, key in contract.items()]
            menus.verify(menu, rows)
            for label, key in contract.items():
                if key is None:
                    for unexpected in (dict(key='Q'), dict(modifiers=1), dict(modifiers=8),
                            dict(virtualKey=53), dict(glyph=27)):
                        bad = [dict(row, **unexpected) if row['label'] == label else row for row in rows]
                        with self.assertRaises(RuntimeError):
                            menus.verify(menu, bad)

    def test_capture_bounds_are_finite_positive_and_enclose_only_the_owned_popup(self):
        self.assertEqual(menus.rectangle('40.5, 25, 250.25, 300.1'), [40, 25, 251, 301])
        for bad in ('-1, 25, 250, 300', '40, 25, nan, 300', '40, 25, 250, inf',
                '40, 25, 250, 0', '40, 25, 2500, 300', '1, 2, 3'):
            with self.assertRaises((RuntimeError, ValueError)):
                menus.rectangle(bad)

    def test_window_commands_must_disable_after_the_last_window_closes(self):
        rows = [dict(label=label, key=key[0] if key else '', modifiers=key[1] if key else -1, enabled=False)
            for label, key in menus.CONTRACT['Window'].items()]
        menus.verify('Window', rows, window_open=False)
        with self.assertRaises(RuntimeError):
            menus.verify('Window', rows, window_open=True)
        rows[0]['enabled'] = True
        with self.assertRaises(RuntimeError):
            menus.verify('Window', rows, window_open=False)

    @unittest.skipUnless(menus.platform.system() == 'Darwin', 'Requires the real System Events dictionary')
    def test_row_accumulator_is_a_local_variable_in_system_events_terms(self):
        # Exercise the actual script prefix without sending any UI AppleEvent.
        # "rows" resolves to System Events' crow class and fails here too.
        prefix = menus.menu_rows_script('Buddy Native').split('repeat with', 1)[0]
        script = ('using terms from application "System Events"\n' + prefix
            + '\nreturn "initialized"\nend using terms from')
        result = menus.subprocess.run(['osascript', '-e', script],
            capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'initialized')

    def test_no_capture_can_target_a_dead_process_or_an_unlisted_menu(self):
        window = Mock()
        window.process.poll.return_value = 0
        with patch.object(menus, 'run') as run:
            with self.assertRaises(RuntimeError):
                menus.capture(window, 'Edit', Path('menu.png'))
            with self.assertRaises(RuntimeError):
                menus.capture(window, 'Other app', Path('menu.png'))
            run.assert_not_called()

    def test_window_wait_tolerates_launch_services_registration_delay(self):
        window = Mock()
        window.process.poll.return_value = None
        with patch.object(menus, 'run', side_effect=[RuntimeError('osascript failed with exit 1'), '1']), \
                patch.object(menus.time, 'sleep'):
            menus.wait_window(window, 1)


class MacMenuFailureState(unittest.TestCase):
    """Portable consumer controls over a fake process boundary. They do not
    exercise real Accessibility, AppKit or System Events behavior."""

    def capture(self, boundary, window, directory, menu='Buddy Native', window_open=True):
        destination = Path(directory) / 'buddy-native.png'
        sleeps = []
        with patch.object(menus, 'run', side_effect=boundary), \
                patch.object(desktop, 'run', side_effect=boundary), \
                patch.object(menus.time, 'sleep', side_effect=sleeps.append):
            try:
                return menus.capture(window, menu, destination, window_open), destination, sleeps
            except Exception as error:
                return error, destination, sleeps

    def test_escape_failure_cannot_replace_the_hosted_bounds_error(self):
        hosted = RuntimeError(HOSTED_861)
        boundary = OwnedDesktop(bounds=hosted, observe='false\t0\t0',
            escape=RuntimeError('osascript failed with exit 1: escape'))
        with tempfile.TemporaryDirectory() as directory:
            raised, _, _ = self.capture(boundary, owned_window(None), directory)
        self.assertIs(raised, hosted)
        self.assertEqual(boundary.names().count('escape'), 1)
        self.assertIn('Native menu Escape also failed (RuntimeError)', ''.join(getattr(hosted, '__notes__', [])))

    def test_hosted_861_replay_records_live_owned_state_before_one_escape(self):
        hosted = RuntimeError(HOSTED_861)
        boundary = OwnedDesktop(bounds=hosted, observe='false\t0\t0')
        with tempfile.TemporaryDirectory() as directory:
            raised, destination, sleeps = self.capture(boundary, owned_window(None), directory)
            snapshot = failure_snapshot(destination)
            self.assertFalse(destination.exists())
            self.assertFalse(destination.with_suffix('.json').exists())
        self.assertIs(raised, hosted)
        self.assertEqual(boundary.names(), ['click', 'bounds', 'observe', 'escape'])
        _, observe, options = boundary.calls[2]
        self.assertEqual(options, dict(timeout=1))
        self.assertEqual(observe[:2], ['osascript', '-e'])
        self.assertTrue(observe[2].startswith('tell application "System Events" to tell first application '
            f'process whose unix id is {PID}\n'))
        for forbidden in ('set frontmost', 'click', 'key code', 'keystroke', 'delay', 'every process'):
            self.assertNotIn(forbidden, observe[2])
        self.assertEqual(sleeps, [0.7])
        self.assertEqual(snapshot, dict(schema='buddy-native-menu-failure/v1', menu='Buddy Native',
            stage='bounds', errorType='RuntimeError', process=dict(state='running', exitCode=None),
            observation=dict(status='observed', **fields(False, 0, 0))))
        self.assertNotIn('76900', menus.json.dumps(snapshot))

    def test_exited_or_unreadable_owner_is_recorded_without_a_state_query(self):
        for poll, process in ((1, dict(state='exited', exitCode=1)), (-9, dict(state='exited', exitCode=-9)),
                (4096, dict(state='exited', exitCode=None)), (OSError('poll'), dict(state='unavailable', exitCode=None)),
                ('0', dict(state='unavailable', exitCode=None))):
            with self.subTest(poll=poll):
                hosted = RuntimeError(HOSTED_861)
                boundary = OwnedDesktop(bounds=hosted)
                with tempfile.TemporaryDirectory() as directory:
                    raised, destination, _ = self.capture(boundary, owned_window(None, poll), directory)
                    snapshot = failure_snapshot(destination)
                self.assertIs(raised, hosted)
                self.assertEqual(boundary.names(), ['click', 'bounds', 'escape'])
                self.assertEqual(snapshot['process'], process)
                self.assertEqual(snapshot['observation'], dict(status='not-run-process-not-running', **fields()))

    def test_each_post_click_stage_keeps_its_original_error(self):
        mismatch = GOOD_ROWS.replace('Quit Buddy Native\tQ\t0', 'Quit Buddy Native\tQ\t8')
        for change, stage, kind in ((dict(rows=RuntimeError('rows')), 'rows', 'RuntimeError'),
                (dict(rows='About Buddy\t\t-1'), 'rows', 'ValueError'),
                (dict(rows=GOOD_ROWS.replace('\ttrue\t', '\tmaybe\t', 1)), 'rows', 'RuntimeError'),
                (dict(bounds='100, 25, 3000, 400'), 'bounds', 'RuntimeError'),
                (dict(screencapture=b'not a png'), 'pixels', 'RuntimeError'),
                (dict(screencapture=subprocess.TimeoutExpired('screencapture', 60)), 'pixels', 'TimeoutExpired'),
                (dict(rows=mismatch), 'verify', 'RuntimeError')):
            with self.subTest(stage=stage, change=change):
                boundary = OwnedDesktop(observe='true\t1\t1', **change)
                with tempfile.TemporaryDirectory() as directory:
                    raised, destination, _ = self.capture(boundary, owned_window(None), directory)
                    snapshot = failure_snapshot(destination)
                    receipt = destination.with_suffix('.json').exists()
                self.assertEqual(type(raised).__name__, kind)
                for value in change.values():
                    if isinstance(value, BaseException):
                        self.assertIs(raised, value)
                self.assertEqual(boundary.names().count('observe'), 1)
                self.assertEqual(boundary.names()[-2:], ['observe', 'escape'])
                self.assertEqual((snapshot['stage'], snapshot['errorType']), (stage, kind))
                self.assertEqual(snapshot['observation'], dict(status='observed', **fields(True, 1, 1)))
                # Only a contract mismatch has read real pixels and rows first.
                self.assertEqual(receipt, stage == 'verify')

    def test_unusable_state_reads_stay_unavailable_and_never_mask_the_failure(self):
        for observe, status, expected in (
                (subprocess.TimeoutExpired('osascript', 1), 'query-timeout', fields()),
                (RuntimeError('osascript failed with exit 1: private title'), 'query-failed', fields()),
                (OSError('osascript'), 'query-failed', fields()),
                ('', 'malformed', fields()), ('true\t1', 'malformed', fields()),
                ('true\t1\t1\t1', 'malformed', fields()), ('yes\t1\t1', 'malformed', fields()),
                ('private title\t1\t1', 'malformed', fields()),
                ('true\t-1\t2', 'observed', fields(True, None, 2)),
                ('unavailable\t1\t-1', 'observed', fields(None, 1, None)),
                ('false\t1000\t1', 'observed', fields(False, None, 1)),
                ('true\t١\t+1', 'observed', fields(True)),
                ('true\t1.0\tprivate', 'observed', fields(True))):
            with self.subTest(observe=observe):
                hosted = RuntimeError(HOSTED_861)
                boundary = OwnedDesktop(bounds=hosted, observe=observe)
                with tempfile.TemporaryDirectory() as directory:
                    raised, destination, _ = self.capture(boundary, owned_window(None), directory)
                    text = destination.with_name('buddy-native.failure.json').read_text()
                self.assertIs(raised, hosted)
                self.assertEqual(boundary.names(), ['click', 'bounds', 'observe', 'escape'])
                self.assertEqual(menus.json.loads(text)['observation'], dict(status=status, **expected))
                self.assertNotIn('private', text)
                self.assertNotIn('osascript', text)

    def test_snapshot_write_and_escape_failures_together_keep_the_original_error(self):
        for observe in ('true\t1\t1', RuntimeError('osascript failed')):
            with self.subTest(observe=observe):
                hosted = RuntimeError(HOSTED_861)
                boundary = OwnedDesktop(bounds=hosted, observe=observe, escape=RuntimeError('escape'))
                with tempfile.TemporaryDirectory() as directory:
                    # A directory in the snapshot's place makes its write fail.
                    (Path(directory) / 'buddy-native.failure.json').mkdir()
                    raised, _, _ = self.capture(boundary, owned_window(None), directory)
                self.assertIs(raised, hosted)
                self.assertEqual(boundary.names(), ['click', 'bounds', 'observe', 'escape'])

    def test_rejected_admission_sends_nothing_and_writes_nothing(self):
        for menu, poll in (('Edit', 0), ('Other app', None)):
            with self.subTest(menu=menu):
                boundary = OwnedDesktop()
                with tempfile.TemporaryDirectory() as directory:
                    raised, _, _ = self.capture(boundary, owned_window(poll), directory, menu=menu)
                    self.assertEqual(list(Path(directory).iterdir()), [])
                self.assertIsInstance(raised, RuntimeError)
                self.assertEqual(boundary.calls, [])

    def test_successful_capture_keeps_its_commands_waits_and_receipt(self):
        boundary = OwnedDesktop()
        with tempfile.TemporaryDirectory() as directory:
            receipt, destination, sleeps = self.capture(boundary, owned_window(None), directory)
            self.assertEqual(sorted(path.name for path in Path(directory).iterdir()),
                ['buddy-native.json', 'buddy-native.png'])
            self.assertEqual(menus.json.loads(destination.with_suffix('.json').read_text()), receipt)
        self.assertEqual(boundary.names(), ['click', 'bounds', 'rows', 'screencapture', 'escape'])
        self.assertEqual([options for _, _, options in boundary.calls], [{}] * 5)
        self.assertEqual(boundary.calls[3][1][:4], ['screencapture', '-x', '-R', '100,25,300,400'])
        self.assertEqual(sleeps, [0.3, 0.7])
        self.assertEqual((receipt['menu'], receipt['bounds'], len(receipt['items'])),
            ('Buddy Native', [100, 25, 300, 400], len(menus.CONTRACT['Buddy Native'])))
        self.assertEqual(receipt['sha256'], menus.hashlib.sha256(b'\x89PNG\r\n\x1a\n').hexdigest())

    def test_receipt_write_failure_is_retained_with_its_own_snapshot(self):
        original = OSError('normal receipt write failed')
        boundary = OwnedDesktop(observe='true\t1\t1', escape=RuntimeError('escape'))
        write = Path.write_text
        def write_receipt(path, *args, **kwargs):
            if path.name == 'buddy-native.json':
                raise original
            return write(path, *args, **kwargs)
        with tempfile.TemporaryDirectory() as directory, patch.object(Path, 'write_text', write_receipt):
            raised, destination, _ = self.capture(boundary, owned_window(None), directory)
            snapshot = failure_snapshot(destination)
            self.assertTrue(destination.exists())
            self.assertFalse(destination.with_suffix('.json').exists())
        self.assertIs(raised, original)
        self.assertEqual((snapshot['stage'], snapshot['errorType']), ('receipt', 'OSError'))
        self.assertEqual(boundary.names(), ['click', 'bounds', 'rows', 'screencapture', 'observe', 'escape'])

    @unittest.skipUnless(menus.platform.system() == 'Darwin', 'Requires the real System Events dictionary')
    def test_failure_state_script_compiles_without_sending_an_apple_event(self):
        window = owned_window(None)
        script = ('using terms from application "System Events"\nif false then\n'
            + window.apple(menus.FAILURE_STATE_SCRIPT)
            + '\nend if\nend using terms from\nreturn "compiled"')
        result = menus.subprocess.run(['osascript', '-e', script],
            capture_output=True, text=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'compiled')

    def test_escape_failure_after_a_good_capture_stays_fatal(self):
        escape = RuntimeError('osascript failed with exit 1: escape')
        boundary = OwnedDesktop(escape=escape)
        with tempfile.TemporaryDirectory() as directory:
            raised, destination, _ = self.capture(boundary, owned_window(None), directory)
            self.assertFalse(destination.with_name('buddy-native.failure.json').exists())
        self.assertIs(raised, escape)
        self.assertEqual(boundary.names(), ['click', 'bounds', 'rows', 'screencapture', 'escape'])


if __name__ == '__main__':
    unittest.main()
