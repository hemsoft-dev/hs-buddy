"""A failed fixture must preserve deployment selectors it does not own."""
import hashlib
import importlib.util
import json
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


def write_convex_cache(cache, version="fixture-1.0", dashboard=True):
    name = "convex-local-backend.exe" if os.name == "nt" else "convex-local-backend"
    binary = Path(cache) / "binaries" / version / name
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"backend")
    if dashboard:
        (Path(cache) / "dashboard/out").mkdir(parents=True)
        (Path(cache) / "dashboard/config.json").write_text(json.dumps({"version": version}))


class ConvexDownloadCache(unittest.TestCase):
    """One job downloads the Convex backend once, not once per scene."""

    def test_only_a_complete_download_is_shared(self):
        with TemporaryDirectory() as temporary:
            partial, shared = Path(temporary) / "partial", Path(temporary) / "shared"
            write_convex_cache(partial, dashboard=False)
            backend.save_convex_cache(partial, shared)
            self.assertFalse(shared.exists())
            complete = Path(temporary) / "complete"
            write_convex_cache(complete)
            backend.save_convex_cache(complete, shared)
            self.assertEqual(backend.cached_backend_version(shared), "fixture-1.0")
            seeded = Path(temporary) / "seeded"
            self.assertEqual(backend.restore_convex_cache(shared, seeded), "fixture-1.0")
            self.assertEqual(backend.cached_backend_version(seeded), "fixture-1.0")
            self.assertIsNone(backend.restore_convex_cache(Path(temporary) / "missing", Path(temporary) / "none"))

    def test_a_failed_cache_copy_falls_back_to_a_normal_download(self):
        with TemporaryDirectory() as temporary:
            shared, seeded = Path(temporary) / "shared", Path(temporary) / "seeded"
            write_convex_cache(shared)
            with patch.object(backend.shutil, "copytree", side_effect=OSError("disk full")):
                self.assertIsNone(backend.restore_convex_cache(shared, seeded))
            self.assertFalse(seeded.exists())

    def test_a_cli_that_exits_before_readiness_gets_one_fresh_retry(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            checkout = Path(temporary) / "checkout"
            checkout.mkdir()
            fixture = backend.SettingsBackend(checkout)
            first_profile = fixture.profile
            launches = []
            def spawn(*args, **kwargs):
                launches.append(fixture.profile)
                if len(launches) == 1:
                    fixture.log.write("Unexpected Error: TypeError: fetch failed")
                    fixture.log.flush()
                    return Mock(pid=123, poll=Mock(return_value=1), wait=Mock(return_value=1))
                fixture.generated.write_text("owned anonymous fixture")
                fixture.log.write("Convex functions ready!")
                fixture.log.flush()
                return Mock(pid=124, poll=Mock(return_value=None))
            with patch.object(backend.shutil, "which", return_value="node"), \
                    patch.object(backend.socket, "socket"), \
                    patch.object(backend.os, "killpg", create=True), \
                    patch.object(backend.subprocess, "run", return_value=Mock(returncode=0)), \
                    patch.object(backend.subprocess, "Popen", side_effect=spawn), \
                    patch.object(backend.SettingsBackend, "query",
                        lambda self, path: [] if path == "githubAccounts:list" else {}):
                try:
                    self.assertIs(fixture.__enter__(), fixture)
                finally:
                    fixture.child = None
                    fixture.__exit__(None, None, None)
            self.assertEqual(len(launches), 2)
            self.assertNotEqual(launches[0], launches[1])
            self.assertIn("fetch failed", (first_profile / "backend.log").read_text())

    def test_no_retry_when_the_failed_attempt_left_its_selector(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            checkout = Path(temporary) / "checkout"
            checkout.mkdir()
            fixture = backend.SettingsBackend(checkout)
            def exited():
                # The CLI rewrote the selector after the fixture's snapshot,
                # so cleanup must preserve it and a retry must not start.
                fixture.generated.write_text("rewritten before the CLI crashed")
                return 1
            def spawn(*args, **kwargs):
                fixture.generated.write_text("written by the CLI")
                return Mock(pid=123, poll=Mock(side_effect=exited), wait=Mock(return_value=1))
            with patch.object(backend.shutil, "which", return_value="node"), \
                    patch.object(backend.socket, "socket"), \
                    patch.object(backend.os, "killpg", create=True), \
                    patch.object(backend.subprocess, "run", return_value=Mock(returncode=0)), \
                    patch.object(backend.subprocess, "Popen", side_effect=spawn) as launched:
                with self.assertRaises(backend.BackendNotReady):
                    fixture.__enter__()
                launched.assert_called_once()
            self.assertTrue(fixture.cleanup_failed)
            self.assertEqual(fixture.generated.read_text(), "rewritten before the CLI crashed")

    def test_later_fixtures_pin_the_cached_version_and_first_one_shares_it(self):
        with TemporaryDirectory() as temporary, patch.dict(os.environ,
                {"GITHUB_ACTIONS": "true", "BUDDY_SETTINGS_FIXTURE": "1", "RUNNER_TEMP": temporary}):
            checkout = Path(temporary) / "checkout"
            checkout.mkdir()
            launches = []
            for _ in range(2):
                fixture = backend.SettingsBackend(checkout)
                def spawn(command, *args, **kwargs):
                    launches.append((command, backend.cached_backend_version(fixture.cache)))
                    if backend.cached_backend_version(fixture.cache) is None:
                        write_convex_cache(fixture.cache)  # The CLI's first download.
                    fixture.generated.write_text("owned anonymous fixture")
                    fixture.log.write("Convex functions ready!")
                    fixture.log.flush()
                    return Mock(pid=123, poll=Mock(return_value=None))
                with patch.object(backend.shutil, "which", return_value="node"), \
                        patch.object(backend.socket, "socket"), \
                        patch.object(backend.subprocess, "Popen", side_effect=spawn), \
                        patch.object(fixture, "query", side_effect=lambda path: [] if path == "githubAccounts:list" else {}):
                    try:
                        fixture.__enter__()
                    finally:
                        fixture.child = None
                        fixture.__exit__(None, None, None)
                self.assertFalse(fixture.cache.exists())
            (first, first_seed), (second, second_seed) = launches
            self.assertNotIn("--local-backend-version", first)
            self.assertIsNone(first_seed)
            self.assertEqual(second[second.index("--local-backend-version") + 1], "fixture-1.0")
            self.assertEqual(second_seed, "fixture-1.0")
            self.assertEqual(second[-4:][0], "--local-cloud-port")


if __name__ == "__main__":
    unittest.main()
