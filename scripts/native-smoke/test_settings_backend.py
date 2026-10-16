"""A failed fixture must preserve deployment selectors it does not own."""
import hashlib
import importlib.util
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("settings_backend", Path(__file__).with_name("settings-backend.py"))
backend = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backend)


class BackendOwnership(unittest.TestCase):
    def test_consecutive_fixtures_do_not_reuse_project_local_database_state(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            checkout = Path(temporary) / "checkout"
            checkout.mkdir()
            state = checkout / ".convex" / "local" / "default"
            for _ in range(2):
                fixture = backend.SettingsBackend(checkout)
                def spawn(*args, **kwargs):
                    state.mkdir(parents=True, exist_ok=True)
                    fixture.generated.write_text("owned anonymous fixture")
                    fixture.log.write("Convex functions ready!")
                    fixture.log.flush()
                    return Mock(pid=123, poll=Mock(return_value=None))
                def query(path):
                    if path == "githubAccounts:list":
                        return ["previous fixture account"] if (state / "records").exists() else []
                    return {}
                with patch.object(backend.shutil, "which", return_value="node"), \
                        patch.object(backend.socket, "socket"), \
                        patch.object(backend.subprocess, "run", return_value=Mock(returncode=0)), \
                        patch.object(backend.subprocess, "Popen", side_effect=spawn), \
                        patch.object(fixture, "query", side_effect=query):
                    try:
                        fixture.__enter__()
                        (state / "records").write_text("fixture accounts")
                    finally:
                        fixture.child = None
                        fixture.__exit__(None, None, None)
                    self.assertEqual((fixture.profile / "backend-state" / "records").read_text(), "fixture accounts")

    def test_existing_project_database_is_preserved_without_starting_a_process(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            state = Path(temporary) / ".convex" / "local" / "default"
            state.mkdir(parents=True)
            (state / "records").write_text("retained deployment")
            with patch.object(backend.subprocess, "Popen") as spawn, \
                    patch.object(backend.subprocess, "run", return_value=Mock(returncode=0)):
                with self.assertRaisesRegex(RuntimeError, "Preserve existing local"):
                    with backend.SettingsBackend(temporary):
                        self.fail("Existing local state is not disposable")
                spawn.assert_not_called()
            self.assertEqual((state / "records").read_text(), "retained deployment")

    def test_each_launch_uses_fresh_loopback_ports_when_previous_ports_are_unavailable(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            fixture = backend.SettingsBackend(temporary)
            sockets = []
            def socket_factory():
                context = unittest.mock.MagicMock()
                probe = context.__enter__.return_value
                def bind(address):
                    if address != ("127.0.0.1", 0):
                        raise OSError("Previous fixture's fixed ports are unavailable")
                probe.bind.side_effect = bind
                probe.getsockname.return_value = ("127.0.0.1", 48130 + len(sockets))
                sockets.append(probe)
                return context
            def spawn(*args, **kwargs):
                self.assertEqual(args[0][-4:], ["--local-cloud-port", "48130", "--local-site-port", "48131"])
                self.assertEqual(fixture.url, "http://127.0.0.1:48130")
                fixture.generated.write_text("owned fixture")
                fixture.log.write("Convex functions ready!")
                fixture.log.flush()
                return Mock(pid=123, poll=Mock(return_value=None))
            with patch.object(backend.shutil, "which", return_value="node"), \
                    patch.object(backend.socket, "socket", side_effect=socket_factory), \
                    patch.object(backend.subprocess, "Popen", side_effect=spawn) as launched, \
                    patch.object(fixture, "query", side_effect=lambda path: [] if path == "githubAccounts:list" else {}):
                try:
                    fixture.__enter__()
                    launched.assert_called_once()
                finally:
                    fixture.child = None
                    fixture.__exit__(None, None, None)

    def test_selector_snapshot_waits_for_completed_cli_setup(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            checkout = Path(temporary) / "checkout"
            checkout.mkdir()
            fixture = backend.SettingsBackend(checkout)
            child = Mock(pid=123, poll=Mock(return_value=None))
            initial = b"CONVEX_DEPLOYMENT=anonymous:fixture\n"
            complete = initial + b"VITE_CONVEX_URL=http://127.0.0.1:8130\n"
            def spawn(*_args, **_kwargs):
                fixture.generated.write_bytes(initial)
                return child
            def query(path):
                if path == "settings:get":
                    fixture.generated.write_bytes(complete)
                    fixture.log.write("Convex functions ready! (0.1s)\n")
                    fixture.log.flush()
                    return {}
                return []
            with patch.object(backend.shutil, "which", return_value="node"), \
                    patch.object(backend.socket, "socket"), \
                    patch.object(backend.subprocess, "Popen", side_effect=spawn), \
                    patch.object(fixture, "query", side_effect=query):
                fixture.__enter__()
            self.assertEqual(fixture.generated_hash, hashlib.sha256(complete).digest())
            fixture.child = None  # The mock never owned an OS process.
            fixture.generated.write_bytes(b"changed after readiness")
            with self.assertRaises(RuntimeError):
                fixture.__exit__(None, None, None)
            self.assertEqual(fixture.generated.read_bytes(), b"changed after readiness")

    def test_normal_developer_environment_cannot_start_a_backend(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "", "BUDDY_SETTINGS_FIXTURE": ""}):
            with self.assertRaises(RuntimeError):
                backend.SettingsBackend(Path.cwd())

    def test_existing_selector_is_preserved_without_starting_a_process(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            checkout = Path(temporary) / "checkout"
            checkout.mkdir()
            selector = checkout / ".env.local"
            selector.write_text("existing deployment")
            with patch.object(backend.subprocess, "Popen") as spawn:
                with self.assertRaises(RuntimeError):
                    with backend.SettingsBackend(checkout):
                        self.fail("An existing selector cannot be taken over")
                spawn.assert_not_called()
            self.assertEqual(selector.read_text(), "existing deployment")

    def test_cleanup_only_removes_unchanged_generated_selector(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            fixture = backend.SettingsBackend(temporary)
            fixture.generated.write_bytes(b"owned selector")
            fixture.generated_hash = hashlib.sha256(b"owned selector").digest()
            fixture.__exit__(None, None, None)
            self.assertFalse(fixture.generated.exists())
            fixture.generated.write_bytes(b"concurrently replaced selector")
            with self.assertRaises(RuntimeError):
                fixture.__exit__(None, None, None)
            self.assertEqual(fixture.generated.read_bytes(), b"concurrently replaced selector")

    def test_cleanup_failure_does_not_replace_primary_error(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            fixture = backend.SettingsBackend(temporary)
            fixture.generated.write_bytes(b"changed")
            fixture.generated_hash = hashlib.sha256(b"original").digest()
            original = ValueError("primary")
            self.assertFalse(fixture.__exit__(ValueError, original, None))
            self.assertTrue(fixture.generated.exists())


if __name__ == "__main__":
    unittest.main()
