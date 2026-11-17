import hashlib
import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

spec = importlib.util.spec_from_file_location('session_workspaces', Path(__file__).with_name('session-workspaces.py'))
workspaces = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workspaces)


class ReadOnlyWorkspaceFixture(unittest.TestCase):
    def prepare(self, output):
        user = output / 'owned User'
        directory = user / 'workspaceStorage' / 'original' / 'chatSessions'
        directory.mkdir(parents=True)
        (directory.parent / 'workspace.json').write_text('{"folder":"file:///synthetic/project"}')
        original = directory / 'original.jsonl'
        original.write_text('{"kind":0,"v":{"sessionId":"original"}}')
        return original, workspaces.prepare(user, original)

    def test_fixture_is_ordered_and_every_source_file_is_verified_without_writes(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            original, files = self.prepare(output)
            self.assertEqual(len(files), 9)
            transcripts = [path for path in files if path.suffix == '.jsonl']
            titles = []
            for path in sorted(transcripts, key=lambda p: p.stat().st_mtime_ns, reverse=True):
                first = json.loads(path.read_text().splitlines()[0])
                titles.append(first['v']['sessionId'])
            self.assertEqual(titles, ['original', 'alpha-new', 'beta', 'alpha-old', 'gamma'])
            before = {path: path.read_bytes() for path in files}
            with workspaces.source_guard(files, output):
                self.assertEqual(hashlib.sha256(original.read_bytes()).hexdigest(), files[original])
            self.assertEqual(before, {path: path.read_bytes() for path in files})
            self.assertEqual(json.loads((output / 'session-workspace-source.json').read_text())['result'], 'passed')

    def test_changed_or_missing_source_refuses_acceptance_without_masking_the_primary_error(self):
        for missing in (False, True):
            with self.subTest(missing=missing), TemporaryDirectory() as temporary:
                output = Path(temporary)
                original, files = self.prepare(output)
                with self.assertRaisesRegex(RuntimeError, 'files changed'):
                    with workspaces.source_guard(files, output):
                        if missing:
                            original.unlink()
                        else:
                            original.write_bytes(b'changed synthetic bytes')
                primary = RuntimeError('original scene failure')
                with self.assertRaises(RuntimeError) as caught:
                    with workspaces.source_guard(files, output):
                        raise primary
                self.assertIs(caught.exception, primary)
                self.assertIn('verification failed', primary.__notes__[0])

    def test_existing_workspace_is_preserved_instead_of_being_reseeded(self):
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            original, files = self.prepare(output)
            before = {path: path.read_bytes() for path in files}
            with self.assertRaises(FileExistsError):
                workspaces.prepare(original.parents[3], original)
            self.assertEqual(before, {path: path.read_bytes() for path in files})
