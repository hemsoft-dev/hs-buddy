import copy
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import Mock, patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('comparison', Path(__file__).with_name('native-comparison.py'))
comparison = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparison)


class BudgetTests(unittest.TestCase):
    def setUp(self):
        row = {'startupMs': 1000, 'responseMedianMs': 30, 'responseMaxMs': 60}
        for scenario in ('dashboard-warm', 'settings-idle', 'navigation-baseline', 'navigation-cleanup', 'peak'):
            row[scenario] = {'rssBytes': 400 * 1024**2, 'privateBytes': None}
        self.summary = {'native': copy.deepcopy(row), 'electron': copy.deepcopy(row)}

    def test_same_runner_control_and_noise_pass(self):
        self.summary['native']['startupMs'] = 2999
        self.summary['native']['peak']['rssBytes'] *= 1.24
        self.assertEqual(comparison.evaluate(self.summary), [])

    def test_regressions_fail(self):
        self.summary['native']['startupMs'] = 3001
        self.summary['native']['responseMedianMs'] = 101
        self.summary['native']['peak']['rssBytes'] *= 1.26
        self.assertEqual(comparison.evaluate(self.summary), ['startupMs', 'responseMedianMs', 'peak.rssBytes'])

    def test_native_cleanup_growth_has_its_own_post_warmup_baseline(self):
        self.summary['native']['navigation-baseline']['rssBytes'] = 64 * 1024**2
        self.summary['native']['navigation-cleanup']['rssBytes'] = 129 * 1024**2
        self.assertEqual(comparison.evaluate(self.summary), ['navigation-growth.rssBytes'])

    def test_missing_private_measurement_cannot_pass(self):
        self.summary['native']['peak']['privateBytes'] = 100
        with self.assertRaises(ValueError):
            comparison.evaluate(self.summary)

    def test_invalid_measurements_cannot_pass(self):
        for value in (0, -1, float('nan'), float('inf'), None):
            with self.subTest(value=value):
                self.summary['native']['startupMs'] = value
                with self.assertRaises(ValueError):
                    comparison.evaluate(self.summary)

    def test_exited_application_never_produces_success(self):
        process = Mock()
        process.is_running.return_value = False
        with self.assertRaisesRegex(RuntimeError, 'exited before receipt'):
            comparison.wait_for(lambda: {'ready': True}, process)

    def test_missing_receipt_times_out(self):
        process = Mock()
        process.is_running.return_value = True
        with self.assertRaises(TimeoutError):
            comparison.wait_for(lambda: None, process, timeout=0.01)

    def test_memory_includes_descendants_and_windows_private_commit(self):
        process, child = Mock(), Mock()
        process.children.return_value = [child]
        process.memory_info.return_value = SimpleNamespace(rss=100, private=80)
        child.memory_info.return_value = SimpleNamespace(rss=50, private=30)
        with patch.object(comparison.os, 'name', 'nt'):
            self.assertEqual(comparison.memory(process), {'rssBytes': 150, 'privateBytes': 110})
        with patch.object(comparison.os, 'name', 'posix'):
            self.assertEqual(comparison.memory(process), {'rssBytes': 150, 'privateBytes': None})

    def test_access_errors_fail_sampling(self):
        process = Mock()
        process.children.return_value = []
        process.memory_info.side_effect = comparison.psutil.AccessDenied()
        with self.assertRaises(comparison.psutil.AccessDenied):
            comparison.memory(process)

    def test_contract_route_list_matches_native_support(self):
        self.assertEqual(len(comparison.ROUTES), 6)
        self.assertEqual(comparison.ROUTES[-1], 'dashboard')


if __name__ == '__main__':
    unittest.main()
