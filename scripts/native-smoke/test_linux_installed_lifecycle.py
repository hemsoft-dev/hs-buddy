"""Failure controls for the installed lifecycle harness, without any installers."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import urllib.request
import contextlib
import json
from urllib.parse import unquote
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("linux_lifecycle", Path(__file__).with_name("linux-installed-lifecycle.py"))
lifecycle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lifecycle)


def restored():
    return dict(projects=[dict(id=lifecycle.PROJECT)],
        session=dict(status="error", conversationHistory=[dict(content=lifecycle.MESSAGE)]),
        cache=dict(data=dict(fixture=lifecycle.MARKER)), viewMode="list")


class BrowserRoutingControl(unittest.TestCase):
    def test_main_renderer_receives_the_actual_bookmark_tab_route(self):
        class Renderer:
            def evaluate(self, expression):
                # A main renderer has the app:navigate listener. No shell API
                # is exposed here: that popup path must fail this control.
                script = "global.CustomEvent=class {constructor(type,init){this.type=type;this.detail=init.detail}};global.window={dispatchEvent:e=>console.log(JSON.stringify(e))};" + expression
                return json.loads(subprocess.check_output(["node", "-e", script], text=True))

        renderer = Renderer()
        page = "http://203.0.113.1:1234/?seed=1&value=a|b"
        with patch.object(renderer, "evaluate", wraps=renderer.evaluate) as evaluate:
            lifecycle.open_browser_tab(renderer, page)
        event = renderer.evaluate(evaluate.call_args.args[0])
        self.assertEqual(event["type"], "app:navigate")
        route = event["detail"]["viewId"]
        self.assertTrue(route.startswith("browser:"))
        encoded, title = route.removeprefix("browser:").split("|")
        self.assertEqual(unquote(encoded), page)
        self.assertEqual(unquote(title), "Lifecycle browser")
        with self.assertRaises(subprocess.CalledProcessError):
            subprocess.check_output(["node", "-e", "global.window={};window.shell.openInAppBrowser('fixture')"], stderr=subprocess.PIPE)


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
    def test_failure_before_baseline_removal_preserves_preceding_installation(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            package = root / "native.deb"
            package.write_bytes(b"canonical package")
            digest = lifecycle.digest(package)
            lifecycle.write(root / "build.json", dict(sourceDirty=False, sourceSha="head", profile="release",
                target="x86_64-unknown-linux-gnu", binarySha256=digest, package=dict(file="native.deb", sha256=digest)))
            packages = lifecycle.Packages(root)
            with patch.object(lifecycle, "require_runner"), \
                    patch.object(lifecycle, "digest", return_value=digest), \
                    patch.object(lifecycle, "load", return_value=unittest.mock.MagicMock()), \
                    patch.object(lifecycle, "browser_fixture", return_value=contextlib.nullcontext({})), \
                    patch.object(lifecycle, "build_electron", side_effect=RuntimeError("early build failure")), \
                    patch.object(lifecycle, "Packages", return_value=packages), \
                    patch.object(packages, "field", return_value="buddy-native"), \
                    patch.object(packages, "remove") as remove, \
                    patch.object(lifecycle.subprocess, "check_output", return_value="head"):
                with self.assertRaisesRegex(RuntimeError, "early build failure"):
                    lifecycle.qualify(root, root / "evidence")
            remove.assert_not_called()
            self.assertEqual(lifecycle.read(root / "evidence/qualification.json")["result"], "failed")

    def test_real_apt_install_must_match_the_deb_version(self):
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw)
            package = output / "canonical.deb"
            package.write_bytes(b"package fixture")
            packages = lifecycle.Packages(output)
            with patch.object(packages, "field", side_effect=["buddy", "1.0"]), \
                    patch.object(packages, "status", return_value="not-installed"), \
                    patch.object(packages, "run") as run, \
                    patch.object(lifecycle.subprocess, "check_output", return_value="0.9"):
                with self.assertRaisesRegex(RuntimeError, "installed deb version"):
                    packages.install(package)
                self.assertEqual(packages.owned, {"buddy"})
                self.assertEqual(run.call_args.args[0], ["sudo", "apt-get", "install", "-y", "--no-install-recommends", "--allow-downgrades", str(package.resolve())])
                packages.remove("buddy")
            self.assertEqual(packages.owned, set())
            self.assertEqual([r["action"] for r in lifecycle.read(output / "packages.json")], ["install-attempt", "install-failed", "remove"])

    def test_partial_apt_failure_remains_owned_for_cleanup(self):
        with tempfile.TemporaryDirectory() as raw:
            output = Path(raw)
            package = output / "canonical.deb"
            package.write_bytes(b"package fixture")
            packages = lifecycle.Packages(output)
            with patch.object(packages, "field", side_effect=["buddy", "1.0"]), \
                    patch.object(packages, "status", return_value="not-installed"), \
                    patch.object(packages, "run", side_effect=RuntimeError("apt failed after unpack")):
                with self.assertRaisesRegex(RuntimeError, "apt failed"):
                    packages.install(package)
            self.assertEqual(packages.owned, {"buddy"})
            with patch.object(packages, "run") as cleanup, patch.object(packages, "status", return_value="config-files"):
                packages.remove("buddy")
            self.assertEqual(packages.owned, set())
            cleanup.assert_called_once_with(["sudo", "apt-get", "remove", "-y", "buddy"])
            self.assertEqual(lifecycle.read(output / "packages.json")[1]["action"], "install-failed")

    def test_existing_installed_or_partial_package_is_never_claimed(self):
        for state in ("installed", "unpacked", "half-configured"):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as raw:
                packages = lifecycle.Packages(Path(raw))
                with patch.object(packages, "field", side_effect=["buddy", "1.0"]), \
                        patch.object(packages, "status", return_value=state), patch.object(packages, "run") as run:
                    with self.assertRaisesRegex(RuntimeError, "Preserve a pre-existing"):
                        packages.install(Path(raw) / "canonical.deb")
                    with self.assertRaisesRegex(RuntimeError, "unowned baseline"):
                        packages.remove("buddy")
                run.assert_not_called()
                self.assertEqual(packages.owned, set())

    def test_successful_command_does_not_hide_failed_package_removal(self):
        with tempfile.TemporaryDirectory() as raw:
            packages = lifecycle.Packages(Path(raw))
            packages.owned.add("buddy")
            with patch.object(packages, "run"), \
                    patch.object(packages, "status", return_value="installed"):
                with self.assertRaisesRegex(RuntimeError, "still installed"):
                    packages.remove("buddy")
            self.assertEqual(packages.records, [])
            self.assertEqual(packages.owned, {"buddy"})

    def test_baseline_removal_requires_explicit_lifecycle_transition(self):
        with tempfile.TemporaryDirectory() as raw:
            packages = lifecycle.Packages(Path(raw))
            with patch.object(packages, "run") as run, patch.object(packages, "status", return_value="not-installed"):
                with self.assertRaisesRegex(RuntimeError, "unowned baseline"):
                    packages.remove("buddy-native")
                run.assert_not_called()
                packages.remove("buddy-native", baseline=True)
            self.assertEqual(packages.records[-1], dict(action="remove", package="buddy-native", baseline=True))
            self.assertEqual(packages.owned, set())

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
