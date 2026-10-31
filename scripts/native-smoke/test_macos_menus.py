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

    def test_no_capture_can_target_a_dead_process_or_an_unlisted_menu(self):
        window = Mock()
        window.process.poll.return_value = 0
        with patch.object(menus, 'run') as run:
            with self.assertRaises(RuntimeError):
                menus.capture(window, 'Edit', Path('menu.png'))
            with self.assertRaises(RuntimeError):
                menus.capture(window, 'Other app', Path('menu.png'))
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
