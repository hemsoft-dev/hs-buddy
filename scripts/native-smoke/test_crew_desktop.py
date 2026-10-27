import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("crew_desktop", Path(__file__).with_name("crew-desktop.py"))
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


def message(role, error=False):
    return dict(id="m", role=role, content="x", timestamp=1, partial=error, error=error)


def crew_state(sessions, projects=("fixture-org/alpha", "fixture-org/beta")):
    return dict(
        projects=[dict(id=f"crew-{index}", githubSlug=slug) for index, slug in enumerate(projects)],
        sessions=sessions,
    )


def session(project, *history, account=desktop.ACCOUNT, model=desktop.MODEL, pending=None):
    return dict(id=f"session-{project}", projectId=project, account=account, model=model,
        conversationHistory=list(history), pending=pending)


class CrewDesktopTests(unittest.TestCase):
    def test_non_runner_is_refused_before_argument_or_desktop_access(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "false", "BUDDY_SETTINGS_FIXTURE": "1"}), \
                patch.object(desktop.argparse, "ArgumentParser") as parser:
            with self.assertRaisesRegex(RuntimeError, "disposable"):
                desktop.main()
            parser.assert_not_called()

    def test_fixture_repositories_name_a_github_origin_with_a_clean_tree(self):
        with tempfile.TemporaryDirectory() as root:
            path = desktop.repository(Path(root) / f"alpha repo {desktop.UNICODE}", "alpha")
            self.assertEqual(desktop.git(path, "remote", "get-url", "origin"),
                "https://github.com/fixture-org/alpha.git")
            self.assertEqual(desktop.git(path, "status", "--porcelain"), "")
            self.assertTrue((path / "src" / "lib.rs").is_file())

    def test_conversations_must_stay_in_their_own_project(self):
        expected = {"fixture-org/alpha": [("user", False), ("assistant", False)], "fixture-org/beta": None}
        good = crew_state([session("crew-0", message("user"), message("assistant"))])
        desktop.verify_conversations(good, expected)
        # Beta's reply landing in alpha's conversation is caught.
        leaked = crew_state([session("crew-0", message("user"), message("assistant"), message("assistant"))])
        with self.assertRaisesRegex(RuntimeError, "fixture-org/alpha conversation"):
            desktop.verify_conversations(leaked, expected)
        other = crew_state([session("crew-0", message("user"), message("assistant"), account="someone-else")])
        with self.assertRaisesRegex(RuntimeError, "changed account"):
            desktop.verify_conversations(other, expected)
        busy = crew_state([session("crew-0", message("user"), message("assistant"),
            pending=dict(request="r", startedAt=1))])
        with self.assertRaisesRegex(RuntimeError, "in flight"):
            desktop.verify_conversations(busy, expected)
        twice = crew_state([session("crew-0", message("user"), message("assistant")),
            dict(session("crew-0"), id="session-2")])
        with self.assertRaisesRegex(RuntimeError, "more than one"):
            desktop.verify_conversations(twice, expected)
        with self.assertRaisesRegex(RuntimeError, "Unexpected Crew projects"):
            desktop.verify_conversations(crew_state([], projects=["fixture-org/alpha"]), expected)

    def test_the_provider_must_have_stayed_offline(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            (root / "copilot-provider-1.json").write_text(json.dumps(dict(
                offline=True, upstreamForwarder=False, requests=2, rejected=1, cancelled=0)), encoding="utf-8")
            (root / "copilot-provider-2.json").write_text(json.dumps(dict(
                offline=True, upstreamForwarder=False, requests=1, rejected=0, cancelled=1)), encoding="utf-8")
            self.assertEqual(desktop.provider_report(root), dict(requests=3, rejected=1, cancelled=1))
            (root / "copilot-provider-3.json").write_text(json.dumps(dict(
                offline=True, upstreamForwarder=True)), encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "not offline"):
                desktop.provider_report(root)


if __name__ == "__main__":
    unittest.main()
