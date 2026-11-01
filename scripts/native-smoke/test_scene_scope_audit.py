"""Keep every scene's view controls out of status/footer and global-tab chrome."""
import ast
import importlib.util
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('scope_ocr', HERE / 'settings-ocr.py')
ocr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ocr)
KINDS = {'workspace', 'click', 'content', 'nav', 'tab', 'between', 'input', 'field', 'box',
    'wait_text', 'wait_content', 'wait_scroll_text', 'wait_scroll_above', 'wait_scroll_up', 'reveal_text'}
SCOPED = {'workspace', 'content', 'nav', 'tab', 'between', 'wait_content'}


def actions():
    for path in HERE.glob('*-desktop.py'):
        for node in ast.walk(ast.parse(path.read_text(encoding='utf-8-sig'))):
            # Callback overrides can locate a control without an action tuple,
            # such as BrowserDriver's tooltip-leave tab lookup.
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == 'locate' and len(node.args) >= 4:
                kind, label = node.args[2:4]
                if isinstance(kind, ast.Constant) and kind.value in KINDS and isinstance(label, ast.Constant) and isinstance(label.value, str):
                    yield path.name, node.lineno, kind.value, label.value
            if isinstance(node, (ast.Tuple, ast.List)) and len(node.elts) >= 2:
                kind, label = node.elts[:2]
                if isinstance(kind, ast.Constant) and kind.value in KINDS and isinstance(label, ast.Constant) and isinstance(label.value, str):
                    yield path.name, node.lineno, kind.value, label.value


class SceneChromeScopes(unittest.TestCase):
    def test_footer_labels_cannot_be_used_as_unscoped_scene_targets(self):
        footer_states = ['0 PRs 0 schedules 0 jobs No gh account',
            'PRs loading schedules loading jobs loading No gh account',
            'PRs unavailable schedules unavailable jobs unavailable No gh account',
            '5 PRs (stale) 2 schedules (stale) 2 jobs (stale) No gh account']
        for scene, line, kind, label in actions():
            for text in footer_states:
                words = [dict(text=token, left=i * 70, top=800, width=60, height=12, line='footer') for i, token in enumerate(text.split())]
                try:
                    ocr.find_label(words, label)
                except ocr.AmbiguousLabel:
                    pass  # Multiple footer matches are still a collision.
                except RuntimeError:
                    continue
                self.assertIn(kind, SCOPED, f'{scene}:{line} {label!r} also identifies footer text')

    def test_browser_controls_and_workspace_titles_have_distinct_scopes(self):
        for scene, line, kind, label in actions():
            if scene != 'browser-desktop.py' or kind not in ('click', 'content', 'workspace'):
                continue
            expected = 'workspace' if label in ('Bookmarks: All', 'Buddy Probe') else 'content'
            self.assertEqual(kind, expected, f'{scene}:{line} {label!r} has the wrong region')

    def test_plain_clicks_are_only_intentional_chrome_or_tempo_boundary_scoped(self):
        chrome = {('automation-desktop.py', 'File')}
        for scene, line, kind, label in actions():
            if kind == 'click' and scene != 'tempo-desktop.py':
                self.assertIn((scene, label), chrome, f'{scene}:{line} view button {label!r} needs content scope')
        # Tempo's platform boundary is exercised separately against the real
        # inherited driver; its view clicks are translated to content actions.


if __name__ == '__main__':
    unittest.main()
