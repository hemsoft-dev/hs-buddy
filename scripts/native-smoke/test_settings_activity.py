"""Require real backend deltas for the installed category-open scene."""
import importlib.util
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import Mock, patch


spec = importlib.util.spec_from_file_location("settings_activity", Path(__file__).with_name("settings-activity.py"))
activity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(activity)


class CategoryActivity(unittest.TestCase):
    def test_exact_delta_rejects_missing_duplicate_and_foreign_events(self):
        activity.require_delta({}, {"tabsOpened": 3}, 3)
        for after in ({"tabsOpened": 0}, {"tabsOpened": 4},
                      {"tabsOpened": 3, "appLaunches": 1},
                      {"tabsOpened": 3, "totalUptimeMs": 1},
                      {"tabsOpened": 3, "lastSessionStart": 1}):
            with self.subTest(after=after), self.assertRaises(RuntimeError):
                activity.require_delta({}, after, 3)

    def test_restart_does_not_count_and_fixture_configuration_is_restored(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)
            config = output / "config.json"
            original = b'{"ui":{"theme":"dark"},"synthetic":"unchanged"}\n'
            config.write_bytes(original)
            backend = Mock()
            backend.query.side_effect = [{}, {"tabsOpened": 3}, {"tabsOpened": 3}]
            driver = Mock()
            driver.drive.side_effect = [output / "dark", output / "light"]
            verify = Mock()
            activity.qualify(driver, backend, config, output, verify)
            self.assertEqual(config.read_bytes(), original)
            self.assertEqual(verify.call_count, 2)
            self.assertEqual(driver.drive.call_args_list[0].args[1][:3], [
                ("nav", "Accounts"), ("nav", "Accounts"), ("nav", "Appearance")])
            self.assertTrue(driver.drive.call_args_list[0].kwargs["record"])
            self.assertNotIn("nav", [item[0] for item in driver.drive.call_args_list[1].args[1]])
            receipt = json.loads((output / "category-activity.json").read_text())
            self.assertEqual((receipt["explicitOpens"], receipt["restartOpens"]), (3, 0))

    def test_failed_counter_or_capture_keeps_the_primary_failure_and_original_config(self):
        for capture_failure in (False, True):
            with self.subTest(capture=capture_failure), TemporaryDirectory() as directory:
                output = Path(directory)
                config = output / "config.json"
                original = b'{"ui":{"theme":"dark"}}'
                config.write_bytes(original)
                backend = Mock()
                backend.query.side_effect = [{}, {"tabsOpened": 2}]
                driver = Mock()
                driver.drive.return_value = output / "capture"
                verify = Mock(side_effect=RuntimeError("missing actual settings content") if capture_failure else None)
                with self.assertRaisesRegex(RuntimeError, "missing actual settings content" if capture_failure else "differs"):
                    activity.qualify(driver, backend, config, output, verify)
                self.assertEqual(config.read_bytes(), original)
                self.assertFalse((output / "category-activity.json").exists())

    def test_cleanup_failure_does_not_hide_the_capture_failure(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)
            config = output / "config.json"
            config.write_text('{"ui":{"theme":"dark"}}')
            backend = Mock()
            backend.query.return_value = {}
            driver = Mock()
            driver.drive.return_value = output / "capture"
            failure = RuntimeError("actual capture failed")
            with patch.object(Path, "write_bytes", side_effect=OSError("synthetic restore failure")):
                with self.assertRaisesRegex(RuntimeError, "actual capture failed") as caught:
                    activity.qualify(driver, backend, config, output, Mock(side_effect=failure))
            self.assertIs(caught.exception, failure)
            self.assertIn("restore failed", caught.exception.__notes__[0])


if __name__ == "__main__":
    unittest.main()
