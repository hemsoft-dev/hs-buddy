"""Bind GitHub evidence to the installed binary and its actual route content."""
import hashlib
import importlib.util
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("github_desktop", Path(__file__).with_name("github-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class InstalledEvidence(unittest.TestCase):
    def test_installed_version_must_match_exactly_before_accepting_its_hash(self):
        with TemporaryDirectory() as temporary:
            executable = Path(temporary) / "owned-binary"
            executable.write_bytes(b"synthetic executable")
            with patch.object(desktop.settings, "run", return_value="Buddy 0.1.13760"):
                with self.assertRaises(RuntimeError):
                    desktop.installed_binary(executable, "0.1.1376")
            with patch.object(desktop.settings, "run", return_value="Buddy 0.1.1376") as run:
                receipt = desktop.installed_binary(executable, "0.1.1376")
            run.assert_called_once_with(executable, "--version")
            self.assertEqual(receipt, dict(versionOutput="Buddy 0.1.1376",
                binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest()))

    def test_route_content_is_required_without_inventing_a_visible_version_label(self):
        with TemporaryDirectory() as temporary:
            image = Path(temporary) / "github.png"
            with patch.object(desktop.settings, "run", return_value="My PRs First private"):
                desktop.check(image, "My PRs", "First private")
            with patch.object(desktop.settings, "run", return_value="My PRs Loading"):
                with self.assertRaises(RuntimeError):
                    desktop.check(image, "My PRs", "First private")
            with patch.object(desktop.settings, "run", return_value="My PRs First private Second private"):
                with self.assertRaisesRegex(RuntimeError, "Another account"):
                    desktop.check(image, "My PRs", "Second private", absent=("First private",))


if __name__ == "__main__":
    unittest.main()
