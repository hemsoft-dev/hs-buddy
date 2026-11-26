import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("automation_desktop", Path(__file__).with_name("automation-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def shell_argv(command):
    """The argv the native exec worker builds for the job's shell."""
    if os.name == "nt":
        return ["pwsh.exe", "-NoProfile", "-NonInteractive", "-Command",
            "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; " + command]
    return ["bash", "-c", command]


class AutomationDesktopTests(unittest.TestCase):
    def test_status_totals_accept_captured_windows_zero_only_for_expected_zero(self):
        words = json.loads((Path(__file__).parent / "fixtures" / "status-session-error-windows.json")
            .read_text(encoding="utf-8"))["words"]
        desktop.verify_status_totals(words, jobs=0, schedules=0)
        for jobs, schedules in ((1, 0), (0, 1)):
            with self.assertRaisesRegex(RuntimeError, "missing"):
                desktop.verify_status_totals(words, jobs, schedules)
        for word in words:
            word["text"] = {"O": "0", "Ojobs": "12jobs"}.get(word["text"], word["text"])
        desktop.verify_status_totals(words, jobs=12, schedules=0)
        with self.assertRaisesRegex(RuntimeError, "missing 2 jobs"):
            desktop.verify_status_totals(words, jobs=2, schedules=0)

    def test_status_totals_reject_body_counts_missing_footer_and_competing_zero_aliases(self):
        words = json.loads((Path(__file__).parent / "fixtures" / "status-session-error-windows.json")
            .read_text(encoding="utf-8"))["words"]
        jobs = next(word for word in words if word["text"] == "Ojobs")
        jobs["top"] -= 100  # A matching count in the content is not the footer total.
        with self.assertRaisesRegex(RuntimeError, "missing 0 jobs"):
            desktop.verify_status_totals(words, 0, 0)
        jobs["top"] += 100
        words.append(dict(jobs, text="0jobs", left=jobs["left"] + 40, line="extra"))
        with self.assertRaises(desktop.settings.ocr.AmbiguousLabel):
            desktop.verify_status_totals(words, 0, 0)
        with self.assertRaisesRegex(RuntimeError, "exactly one native status footer"):
            desktop.verify_status_totals([], 0, 0)

    def test_status_totals_cannot_combine_ocr_layouts_and_engine_errors_propagate(self):
        backend = Mock()
        backend.query.return_value = []
        words = json.loads((Path(__file__).parent / "fixtures" / "status-session-error-windows.json")
            .read_text(encoding="utf-8"))["words"]
        without_jobs = [word for word in words if word["text"] != "Ojobs"]
        without_schedules = [word for word in words if word["text"] != "schedules"]
        with tempfile.TemporaryDirectory() as root:
            image = Path(root) / "status.png"
            with patch.object(desktop.github, "analysis_images", return_value=[image]), \
                    patch.object(desktop.settings.ocr, "recognize",
                        side_effect=[without_jobs, without_schedules, without_jobs]):
                with self.assertRaisesRegex(RuntimeError, "both backend totals"):
                    desktop.check_status_totals(image, backend)
            with patch.object(desktop.github, "analysis_images", return_value=[image]), \
                    patch.object(desktop.settings.ocr, "recognize", side_effect=OSError("OCR failed")):
                with self.assertRaises(OSError):
                    desktop.check_status_totals(image, backend)

    def test_non_runner_is_refused_before_argument_file_backend_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser, \
                patch.object(desktop.settings, "SettingsBackend") as backend:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()
            backend.assert_not_called()

    def test_echo_job_runs_in_its_shell_with_unicode_directory_and_environment(self):
        with tempfile.TemporaryDirectory() as root:
            workspace = Path(root) / "workspace with spaces 資料"
            workspace.mkdir()
            proof = Path(root) / "proof.json"
            arguments = desktop.job_arguments("Echo job", desktop.echo_command(proof, "nonce-1"), workspace)
            environment = dict(os.environ, **arguments["config"]["env"])
            flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
            result = subprocess.run(shell_argv(arguments["config"]["command"]), cwd=workspace, env=environment,
                capture_output=True, timeout=60, creationflags=flags)
            self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", "replace"))
            output = result.stdout.decode("utf-8")
            self.assertIn("NATIVEAUTOMATIONREADY", output)
            self.assertIn(desktop.UNICODE, output)
            self.assertNotIn("NATIVEAUTOMATIONREADY", arguments["config"]["command"])
            desktop.verify_proof(proof, "nonce-1", workspace)
            with self.assertRaisesRegex(RuntimeError, "did not run once"):
                desktop.verify_proof(proof, "another-run", workspace)
            with self.assertRaisesRegex(RuntimeError, "did not run once"):
                desktop.verify_proof(proof, "nonce-1", Path(root))

    def test_runs_must_be_single_and_in_the_expected_state(self):
        backend = Mock()
        backend.query.return_value = [dict(jobId="job", status="completed",
            output=dict(stdout="NATIVEAUTOMATIONREADY\n資料界é\n", stderr="", exitCode=0)),
            dict(jobId="other", status="failed")]
        desktop.verify_output(desktop.single_run(backend, "job", "completed"))
        with self.assertRaisesRegex(RuntimeError, "cancelled"):
            desktop.single_run(backend, "job", "cancelled")
        backend.query.return_value.append(dict(jobId="job", status="completed"))
        with self.assertRaisesRegex(RuntimeError, "exactly one run"):
            desktop.single_run(backend, "job", "completed")
        for output in (None, "NATIVEAUTOMATIONREADY 資料界é", {"stdout": "NATIVEAUTOMATIONREADY"},
                {"stdout": "NATIVEAUTOMATIONREADY 資料界é", "exitCode": 1}):
            with self.assertRaisesRegex(RuntimeError, "Unicode output"):
                desktop.verify_output(dict(output=output))

    def test_a_surviving_cancelled_process_fails_and_the_sentinel_is_checked(self):
        with tempfile.TemporaryDirectory() as root:
            receipts = Path(root) / "children.jsonl"
            receipts.write_text(json.dumps(dict(run="old", pids=[5, 6])) + "\n"
                + json.dumps(dict(run="now", pids=[7, 8])) + "\n", encoding="utf-8")
            sentinel = Mock()
            sentinel.poll.return_value = None
            with patch.object(desktop.terminal, "pid_running", return_value=False):
                desktop.verify_cancelled_children(receipts, sentinel, "now")
            with patch.object(desktop.terminal, "pid_running", side_effect=lambda pid: pid == 8), \
                    patch.object(desktop.time, "monotonic", side_effect=[0, 0, 11]), \
                    patch.object(desktop.time, "sleep"):
                with self.assertRaisesRegex(RuntimeError, "survived"):
                    desktop.verify_cancelled_children(receipts, sentinel, "now")
            sentinel.poll.return_value = 1
            with patch.object(desktop.terminal, "pid_running", return_value=False):
                with self.assertRaisesRegex(RuntimeError, "sentinel exited"):
                    desktop.verify_cancelled_children(receipts, sentinel, "now")
            with self.assertRaisesRegex(RuntimeError, "exactly one receipt"):
                desktop.verify_cancelled_children(receipts, sentinel, "missing")

    def test_the_dark_pass_captures_cards_then_lists_and_the_light_pass_returns_to_cards(self):
        captures = [step[1] for step in desktop.RUN_VIEW_DARK_STEPS if step[0] == "capture"]
        self.assertEqual(captures, ["run-history-cards-dark", "run-history-list-dark"])
        clicks = [step[1] for step in desktop.RUN_VIEW_DARK_STEPS if step[0] == "content"]
        self.assertEqual(clicks, ["List view"])
        # The layout settles before the cards are captured and the toggle is located.
        self.assertEqual(desktop.RUN_VIEW_DARK_STEPS[:3],
            [("wait_text", "Dispatcher idle"), ("sleep", 1), ("capture", "run-history-cards-dark")])
        self.assertEqual(desktop.RUN_VIEW_LIGHT_STEPS[0], ("content", "Card view"))
        self.assertEqual(desktop.RUN_VIEW_LIGHT_STEPS[-1], ("capture", "run-history-cards-light"))
        with patch.object(desktop.github, "check") as check:
            desktop.check_run_views(Path("runs"))
        cards, listed = check.call_args_list
        self.assertIn("Hide output", cards.args)
        self.assertEqual(cards.kwargs["absent"], ("Triggered",))
        for word in ("Long job", "Cancelled", *desktop.LIST_HEADER):
            self.assertIn(word, listed.args)
        self.assertIn("Hide output", listed.kwargs["absent"])

    def test_the_layout_must_be_saved_locally_and_synced_to_convex(self):
        backend = Mock()
        with tempfile.TemporaryDirectory() as root:
            config = Path(root) / "config.json"
            config.write_text(json.dumps(dict(native=dict(automation=dict(viewModes={"run-list": "list"})))),
                encoding="utf-8")
            backend.query.return_value = dict(viewModes={"run-list": "list"})
            desktop.require_run_view(config, backend, "list")
            backend.query.assert_called_with("settings:get")
            with self.assertRaisesRegex(RuntimeError, "config.json"):
                desktop.require_run_view(config, backend, "card")
            backend.query.return_value = dict(viewModes={})
            with self.assertRaisesRegex(RuntimeError, "Convex"):
                desktop.require_run_view(config, backend, "list")
            backend.query.return_value = None
            with self.assertRaisesRegex(RuntimeError, "Convex"):
                desktop.require_run_view(config, backend, "list")
            config.write_text("{}", encoding="utf-8")
            self.assertIsNone(desktop.saved_run_view(config))

    def test_restoring_returns_both_stores_to_cards_and_keeps_other_settings(self):
        backend = Mock()
        with tempfile.TemporaryDirectory() as root:
            config = Path(root) / "config.json"
            config.write_text(json.dumps(dict(ui=dict(theme="light"), native=dict(automation=dict(
                future=1, viewModes={"run-list": "list", "other": "card"})))), encoding="utf-8")
            with patch.object(desktop.github, "mutate") as mutate:
                desktop.restore_run_view(config, backend)
                desktop.restore_run_view(config, backend)
            value = json.loads(config.read_text(encoding="utf-8"))
            self.assertEqual(value["native"]["automation"], dict(future=1, viewModes={"other": "card"}))
            self.assertEqual(value["ui"], dict(theme="light"))
            mutate.assert_called_with(backend, "settings:updateViewMode", dict(pageKey="run-list", mode="card"))
            config.write_text("{}", encoding="utf-8")
            with patch.object(desktop.github, "mutate"):
                desktop.restore_run_view(config, backend)

    def test_a_failed_scene_still_restores_the_layout_and_reports_its_own_failure(self):
        with tempfile.TemporaryDirectory() as root, \
                patch.object(desktop.github, "installed_binary", return_value={}), \
                patch.object(desktop, "fixture", return_value=Path(root) / "config.json"), \
                patch.object(desktop.terminal, "write_config"), \
                patch.object(desktop.settings, "Driver"), \
                patch.object(desktop.terminal, "start_sentinel"), \
                patch.object(desktop.terminal, "stop_sentinel") as stop, \
                patch.object(desktop, "qualify_offline"), \
                patch.object(desktop, "qualify_runs", side_effect=RuntimeError("scene failed")), \
                patch.object(desktop, "restore_run_view", side_effect=OSError("restore failed")) as restore:
            with self.assertRaisesRegex(RuntimeError, "scene failed"):
                desktop.qualify(Path("buddy"), Path(root), "x11", "1.0", Mock())
            restore.assert_called_once()
            stop.assert_called_once()
            with patch.object(desktop, "qualify_runs", return_value=("echo", "long")), \
                    patch.object(desktop, "require_run_view"), \
                    patch.object(desktop, "qualify_editing"), \
                    patch.object(desktop.terminal, "verify_sentinel"):
                # With no scene failure to report, a failed restore is the failure.
                second = Path(root) / "second"
                second.mkdir()
                with self.assertRaisesRegex(OSError, "restore failed"):
                    desktop.qualify(Path("buddy"), second, "x11", "1.0", Mock())


if __name__ == "__main__":
    unittest.main()
