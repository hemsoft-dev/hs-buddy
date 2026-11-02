"""Portable layout evidence failure controls; no application or installer."""
import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location("lifecycle_layout", Path(__file__).with_name("linux-lifecycle-layout.py"))
layout = importlib.util.module_from_spec(spec)
spec.loader.exec_module(layout)


def config(panes):
    return dict(ui=dict(paneSizes=panes, assistantOpen=False))


def sample():
    return dict(config=config([307, 845, 350]), viewport=dict(width=1200, height=820),
        outer=dict(width=1200, height=820), dpr=1,
        body=dict(x=0, y=30, width=1200, height=768), activity=dict(x=0, y=30, width=48, height=768),
        split=dict(x=48, y=30, width=1152, height=768),
        panes=[dict(x=48, y=30, width=307, height=768), dict(x=355, y=30, width=845, height=768)])


class LayoutEvidenceControls(unittest.TestCase):
    def test_non_numeric_and_malformed_persisted_vectors_are_refused(self):
        original = config([320, 880])
        for panes in (None, {}, [307, 845], [True, 845, 350], [float("nan"), 845, 350],
                [307, float("inf"), 350], [307, 845, -1], [307, 845, 350, 1]):
            with self.subTest(panes=panes), self.assertRaises(RuntimeError):
                layout.validate_sample(original, config(panes), sample())

    def test_collector_observes_debounce_and_writes_both_receipts(self):
        with tempfile.TemporaryDirectory() as raw:
            directory = Path(raw)
            writes = {}
            cdp = Mock(evaluate=lambda _: copy.deepcopy(sample()))
            def wait_for(read, process, timeout):
                self.assertEqual(timeout, 10)
                return read()
            with patch.object(layout.time, "sleep") as sleep, patch.object(layout.time, "monotonic", side_effect=[1, 1.4]):
                result = layout.collect(cdp, object(), wait_for, config([320, 880]), directory,
                    "electron-seed", lambda path, value: writes.update({path.name: value}))
            sleep.assert_called_once_with(.4)
            self.assertEqual(result["samples"][1]["observedAtMs"] - result["samples"][0]["observedAtMs"], 400)
            self.assertEqual(set(writes), {"layout-latest.json", "layout-observation.json"})

    def test_unsettled_failure_retains_observations_without_a_success_receipt(self):
        with tempfile.TemporaryDirectory() as raw:
            writes = {}
            invalid = sample()
            invalid["panes"] = []
            cdp = Mock(evaluate=lambda _: copy.deepcopy(invalid))
            def timed_out(read, process, timeout):
                self.assertEqual(timeout, 10)
                self.assertFalse(read())
                raise TimeoutError("controlled layout timeout")
            with patch.object(layout.time, "sleep"), patch.object(layout.time, "monotonic", side_effect=[1, 1.4]):
                with self.assertRaisesRegex(TimeoutError, "controlled layout timeout"):
                    layout.collect(cdp, object(), timed_out, config([320, 880]), Path(raw),
                        "electron-seed", lambda path, value: writes.update({path.name: value}))
            self.assertEqual(set(writes), {"layout-latest.json"})
            self.assertEqual(writes["layout-latest.json"]["samples"][1]["panes"], [])


if __name__ == "__main__":
    unittest.main()
