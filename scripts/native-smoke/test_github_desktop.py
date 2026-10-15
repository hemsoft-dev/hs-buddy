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
    def test_diff_markers_must_match_exactly_in_one_native_ocr_layout(self):
        outputs = {
            11: "stc/navigation.s native navigation Patch unavailable",
            3: "src/navigationrs native navigation Patch unavailable",
            6: "src/navigation.rs native navigation Patch unavailable",
        }
        modes = []

        def recognize(*arguments, **kwargs):
            mode = int(arguments[arguments.index("--psm") + 1])
            modes.append(mode)
            self.assertEqual(kwargs.get("encoding"), "utf-8")
            return outputs[mode]

        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=recognize):
            image = Path(temporary) / "diff.png"
            desktop.check(image, "navigation.rs", "native navigation", "Patch unavailable")
            self.assertIn("src/navigation.rs", image.with_suffix(".ocr.txt").read_text(encoding="utf-8"))
        self.assertEqual(modes, [11, 3, 6])

    def test_partial_markers_from_different_layouts_cannot_be_combined(self):
        outputs = {11: "navigation.rs", 3: "native navigation", 6: "Patch unavailable"}
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=lambda *args, **_: outputs[int(args[args.index("--psm") + 1])]):
            with self.assertRaises(RuntimeError):
                desktop.check(Path(temporary) / "diff.png", "navigation.rs", "native navigation", "Patch unavailable")

    def test_cross_account_data_cannot_be_retried_away_by_another_ocr_layout(self):
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", return_value="First private Second private") as recognize:
            with self.assertRaisesRegex(RuntimeError, "Another account"):
                desktop.check(Path(temporary) / "github.png", "First private", absent=("Second private",))
            self.assertEqual(recognize.call_count, 1)

    def test_ocr_engine_failure_is_not_treated_as_missing_text(self):
        with TemporaryDirectory() as temporary, patch.object(desktop.settings, "run", side_effect=RuntimeError("OCR engine failed")) as recognize:
            with self.assertRaisesRegex(RuntimeError, "OCR engine failed"):
                desktop.check(Path(temporary) / "github.png", "First private")
            self.assertEqual(recognize.call_count, 1)

    def test_utf8_ocr_receipts_are_read_independently_of_the_windows_code_page(self):
        read_text = Path.read_text

        def windows_default(path, *args, **kwargs):
            kwargs.setdefault("encoding", "cp1252")
            return read_text(path, *args, **kwargs)

        with TemporaryDirectory() as temporary:
            image = Path(temporary) / "github.png"
            with patch.object(desktop.settings, "run", return_value="First private List \u201d"), patch.object(
                Path, "read_text", autospec=True, side_effect=windows_default
            ):
                desktop.check(image, "First private", "List")
            self.assertEqual(image.with_suffix(".ocr.txt").read_text(encoding="utf-8"), "first private list \u201d")

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
