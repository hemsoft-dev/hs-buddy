"""A failed fixture must preserve deployment selectors it does not own."""
import hashlib
import importlib.util
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("settings_backend", Path(__file__).with_name("settings-backend.py"))
backend = importlib.util.module_from_spec(spec)
spec.loader.exec_module(backend)


class BackendOwnership(unittest.TestCase):
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
