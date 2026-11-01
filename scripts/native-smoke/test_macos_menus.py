"""Guard native menu captures and reject incorrect platform accelerators."""
import importlib.util
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('macos_menus', Path(__file__).with_name('macos-menus.py'))
menus = importlib.util.module_from_spec(spec)
spec.loader.exec_module(menus)


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


if __name__ == '__main__':
    unittest.main()
