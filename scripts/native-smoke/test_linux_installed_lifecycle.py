"""Failure controls for the installed lifecycle harness, without any installers."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import urllib.request
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("linux_lifecycle", Path(__file__).with_name("linux-installed-lifecycle.py"))
lifecycle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lifecycle)


def restored():
    return dict(projects=[dict(id=lifecycle.PROJECT)],
        session=dict(status="error", conversationHistory=[dict(content=lifecycle.MESSAGE)]),
        cache=dict(data=dict(fixture=lifecycle.MARKER)), viewMode="list")


class DataFailureControls(unittest.TestCase):
    def test_unknown_fields_must_survive_while_new_defaults_are_allowed(self):
        expected = dict(accounts=[dict(username="fixture", futureAccount=dict(preserve=True))], futureTop=[1, None])
        lifecycle.require_subset(expected, dict(expected, newDefault=False))
        lifecycle.require_subset(expected, dict(accounts=[dict(username="fixture", futureAccount=dict(preserve=True), usageProvider="copilot")], futureTop=[1, None]))
        for changed in (dict(accounts=[], futureTop=[1, None]), dict(accounts=expected["accounts"]),
                dict(accounts=[dict(username="fixture")], futureTop=[1, None]),
                dict(accounts=[dict(username="fixture", futureAccount=dict(preserve=1))], futureTop=[1, None])):
            with self.assertRaises(RuntimeError):
                lifecycle.require_subset(expected, changed)

    def test_real_service_readbacks_are_required_not_just_files(self):
        lifecycle.restored_state(restored())
        for key in ("projects", "session", "cache", "viewMode"):
            value = restored()
            value.pop(key)
            with self.subTest(key=key), self.assertRaises(RuntimeError):
                lifecycle.restored_state(value)

    def test_lost_conversation_and_recovery_status_fail(self):
        for session in (dict(status="idle", conversationHistory=[dict(content=lifecycle.MESSAGE)]),
                dict(status="error", conversationHistory=[]), dict(status="error", conversationHistory=[dict(content="wrong")])):
            value = restored()
            value["session"] = session
            with self.assertRaises(RuntimeError):
                lifecycle.restored_state(value)

    def test_developer_environment_is_refused_before_install_commands(self):
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "", "BUDDY_SETTINGS_FIXTURE": ""}), \
                patch.object(lifecycle.subprocess, "run") as run:
            with self.assertRaisesRegex(RuntimeError, "owned Ubuntu"):
                lifecycle.qualify(Path("unused"), Path("unused"))
            run.assert_not_called()

    def test_browser_origin_is_served_by_the_owned_loopback_proxy(self):
        with lifecycle.browser_fixture() as browser:
            self.assertTrue(browser["url"].startswith("http://203.0.113.1:"))
            self.assertTrue(browser["proxy"].startswith("http://127.0.0.1:"))
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({"http": browser["proxy"]}))
            with opener.open(browser["url"], timeout=2) as response:
                text = response.read().decode()
            self.assertIn(lifecycle.MARKER, text)
            self.assertIn("localStorage", text)


class CanonicalPackageControls(unittest.TestCase):
    def test_real_apt_install_must_match_the_deb_version(self):
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw)
            package = output / "canonical.deb"
            package.write_bytes(b"package fixture")
            packages = lifecycle.Packages(output)
            with patch.object(packages, "field", side_effect=["buddy", "1.0"]), \
                    patch.object(packages, "run") as run, \
                    patch.object(lifecycle.subprocess, "check_output", return_value="0.9"):
                with self.assertRaisesRegex(RuntimeError, "installed deb version"):
                    packages.install(package)
            self.assertEqual(run.call_args.args[0], ["sudo", "apt-get", "install", "-y", "--no-install-recommends", "--allow-downgrades", str(package.resolve())])
            self.assertEqual(packages.records, [])

    def test_successful_command_does_not_hide_failed_package_removal(self):
        with tempfile.TemporaryDirectory() as raw:
            packages = lifecycle.Packages(Path(raw))
            with patch.object(packages, "run"), \
                    patch.object(lifecycle.subprocess, "run", return_value=Mock(stdout="install ok installed")):
                with self.assertRaisesRegex(RuntimeError, "still installed"):
                    packages.remove("buddy")
            self.assertEqual(packages.records, [])

    def test_installer_is_not_substituted_with_extraction(self):
        source = Path(__file__).with_name("linux-installed-lifecycle.py").read_text()
        self.assertNotIn('"dpkg-deb", "-x"', source)
        self.assertNotIn('"--no-sandbox"', source)
        workflow = Path(__file__).resolve().parents[2] / ".github/workflows/rust.yml"
        step = workflow.read_text().split("- name: Qualify Ubuntu installed runtime upgrade and rollback", 1)[1].split("\n      - name:", 1)[0]
        self.assertIn("if: runner.os == 'Linux'", step)
        self.assertIn("todoist-secret-session.sh xvfb-run", step)


if __name__ == "__main__":
    unittest.main()
