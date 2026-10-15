"""Bind GitHub evidence to the installed binary and its actual route content."""
import hashlib
import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("github_desktop", Path(__file__).with_name("github-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class InstalledEvidence(unittest.TestCase):
    def test_failed_navigation_preserves_the_observed_requests(self):
        requests = self.detail_requests()
        with TemporaryDirectory() as temporary:
            output = Path(temporary)
            desktop.preserve_failed_requests(output, requests, RuntimeError("original navigation failure"))
            self.assertEqual(json.loads((output / "requests.json").read_text()), requests)

    def test_diagnostic_write_failure_does_not_replace_the_original_failure(self):
        output = Mock()
        output.__truediv__ = Mock(return_value=Mock(write_text=Mock(side_effect=OSError("unavailable"))))
        failure = RuntimeError("original navigation failure")
        desktop.preserve_failed_requests(output, self.detail_requests(), failure)
        self.assertEqual(str(failure), "original navigation failure")
        self.assertEqual(failure.__notes__, ["Request diagnostic failed: OSError"])

    def detail_requests(self):
        graphql = [dict(account="fixture-one", method="POST", path="/graphql", query="query " + operation)
            for operation in ("NativePullComments", "NativePullReviews", "NativePullCommits", "NativePullThreads", "NativeThreadComments")]
        rest = [dict(account="fixture-one", method="GET", path="/repos/fixture-org/native-demo" + path)
            for path in ("/pulls/7/files", "/commits/head/check-runs", "/commits/head/statuses", "/issues/7/timeline")]
        return graphql + rest

    def test_click_receipts_cannot_substitute_for_loading_the_review_tab(self):
        requests = self.detail_requests()
        desktop.verify_detail_requests(requests)
        requests = [request for request in requests if "NativePullReviews" not in request.get("query", "")]
        with self.assertRaisesRegex(RuntimeError, "NativePullReviews"):
            desktop.verify_detail_requests(requests)

    def test_another_accounts_detail_requests_do_not_qualify_the_selected_account(self):
        requests = [dict(request, account="fixture-two") for request in self.detail_requests()]
        with self.assertRaises(RuntimeError):
            desktop.verify_detail_requests(requests)

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
