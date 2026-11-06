import ast
import copy
import importlib.util
import json
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path
import unittest
from unittest.mock import Mock, patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('comparison', Path(__file__).with_name('native-comparison.py'))
comparison = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparison)
ROOT = Path(__file__).resolve().parents[1]


class BudgetTests(unittest.TestCase):
    def setUp(self):
        row = {'startupMs': 1000, 'responseMedianMs': 30, 'responseMaxMs': 60, 'maxSamplingGapMs': 150}
        for scenario in ('dashboard-warm', 'settings-idle', 'navigation-baseline', 'navigation-cleanup', 'peak'):
            row[scenario] = {'rssBytes': 400 * 1024**2, 'privateBytes': None}
        self.summary = {'native': copy.deepcopy(row), 'electron': copy.deepcopy(row)}

    def test_same_runner_control_and_noise_pass(self):
        self.summary['native']['startupMs'] = 2999
        self.summary['native']['peak']['rssBytes'] *= 1.24
        self.assertEqual(comparison.evaluate(self.summary), [])

    def test_sparse_sampling_cannot_qualify_either_runtime(self):
        self.summary['native']['maxSamplingGapMs'] = 5001
        self.summary['electron']['maxSamplingGapMs'] = 5001
        self.assertEqual(comparison.evaluate(self.summary), ['native.sampling-gap', 'electron.sampling-gap'])

    def test_warmup_route_times_are_retained_but_excluded_from_steady_response(self):
        trace = [{'cycle': 0, 'ms': 55000}, {'cycle': 1, 'ms': 10}, {'cycle': 1, 'ms': 30}]
        self.assertEqual(comparison.response_metrics(trace), {'responseMedianMs': 20, 'responseMaxMs': 30})
        self.assertEqual(len(trace), 3)
        with self.assertRaises(ValueError):
            comparison.response_metrics(trace[:1])

    def test_sampling_coverage_includes_initial_and_trailing_gaps(self):
        self.assertAlmostEqual(comparison.sampling_gap([{'atSeconds': 2}, {'atSeconds': 2.1}], 2.2), 2000)
        self.assertAlmostEqual(comparison.sampling_gap([{'atSeconds': .1}, {'atSeconds': .2}], 3), 2800)
        with self.assertRaises(ValueError):
            comparison.sampling_gap([], 3)

    def test_packaged_control_hashes_the_application_archive(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            self.assertIsNone(comparison.electron_asar(directory / 'Buddy.exe'))
            (directory / 'resources').mkdir()
            archive = directory / 'resources/app.asar'
            archive.write_bytes(b'packaged application')
            self.assertEqual(comparison.electron_asar(directory / 'Buddy.exe'), archive)
            contents = directory / 'Buddy.app/Contents'
            (contents / 'MacOS').mkdir(parents=True)
            (contents / 'Resources').mkdir()
            mac_archive = contents / 'Resources/app.asar'
            mac_archive.write_bytes(b'packaged application')
            self.assertEqual(comparison.electron_asar(contents / 'MacOS/Buddy'), mac_archive)

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


def native_trace_receipt(request_id, *, previous=True, **changes):
    """One complete receipt, as the Windows probe writes it."""
    trace = {'schema': 1, 'available': True, 'id': request_id,
             'pollGapMs': 5.25, 'readMs': .1, 'removeDoneMs': .2, 'updateEntryMs': .3,
             'setSectionDoneMs': 16.0, 'firstFrameMs': 40.0, 'secondFrameMs': 72.5,
             'settings': {'lockMs': 1.0, 'lockCalls': 1, 'loadMs': 2.0, 'loadCalls': 1,
                          'saveMs': 9.0, 'saveCalls': 1, 'installMs': .5, 'installCalls': 1,
                          'attempts': 1},
             'previousId': request_id - 1 if previous else None,
             'previousWriteMs': .75 if previous else None}
    trace.update(changes)
    return {'id': request_id, 'status': 'ok', 'viewport': {'width': 1200, 'height': 820, 'scale': 1}, 'trace': trace}


class NativeTraceContractTests(unittest.TestCase):
    def check(self, trace_or_text, request_id=7):
        text = trace_or_text if isinstance(trace_or_text, str) else json.dumps(trace_or_text)
        return comparison.native_trace(text, request_id)

    def test_complete_trace_keeps_the_frame_gap_as_one_combined_bucket(self):
        trace, reason = self.check(native_trace_receipt(7))
        self.assertIsNone(reason)
        self.assertEqual(trace['firstToSecondFrameMs'], 32.5)
        self.assertEqual(trace['settings']['saveMs'], 9.0)
        self.assertEqual(trace['previousId'], 6)
        self.assertEqual(trace['previousWriteMs'], .75)
        self.assertNotIn('residualMs', trace)

    def test_first_receipt_has_no_previous_write(self):
        trace, reason = self.check(native_trace_receipt(1, previous=False), 1)
        self.assertIsNone(reason)
        self.assertIsNone(trace['previousId'])
        self.assertIsNone(trace['previousWriteMs'])

    def test_unusable_traces_are_unavailable_with_one_fixed_reason(self):
        good = native_trace_receipt(7)
        bad_stage = copy.deepcopy(good)
        bad_stage['trace']['settings']['saveMs'] = 999.0
        extra_stage = copy.deepcopy(good)
        extra_stage['trace']['settings']['note'] = 1
        no_trace = {k: v for k, v in good.items() if k != 'trace'}
        cases = {
            'missing': no_trace,
            'not an object': good | {'trace': 'text'},
            'wrong schema': native_trace_receipt(7, schema=2),
            'extra field': native_trace_receipt(7, path='C:/private/config.json'),
            'wrong id': native_trace_receipt(7, id=8),
            'string id': native_trace_receipt(7, id='7'),
            'boolean id': native_trace_receipt(7, id=True),
            'negative': native_trace_receipt(7, readMs=-1),
            'boolean span': native_trace_receipt(7, readMs=True),
            'string span': native_trace_receipt(7, readMs='1'),
            'above bound': native_trace_receipt(7, readMs=comparison.TRACE_MAX_MS + 1),
            'out of order': native_trace_receipt(7, firstFrameMs=80.0),
            'stages exceed their span': bad_stage,
            'stage field': extra_stage,
            'wrong previous': native_trace_receipt(7, previousId=3),
            'previous id alone': native_trace_receipt(7, previousWriteMs=None),
            'previous write alone': native_trace_receipt(7, previousId=None),
            'previous on first': native_trace_receipt(1, id=1),
        }
        expected = {'missing': 'missing', 'not an object': 'malformed', 'wrong schema': 'malformed',
                    'extra field': 'malformed', 'wrong id': 'wrong-id', 'string id': 'wrong-id', 'boolean id': 'wrong-id',
                    'stage field': 'unhealthy'}
        for name, receipt in cases.items():
            with self.subTest(name):
                request_id = 1 if name == 'previous on first' else 7
                trace, reason = self.check(receipt, request_id)
                self.assertIsNone(trace)
                self.assertEqual(reason, expected.get(name, 'unhealthy'))

    def test_text_that_python_would_accept_is_refused_by_the_strict_parse(self):
        text = json.dumps(native_trace_receipt(7))
        cases = {
            'NaN': text.replace('"readMs": 0.1', '"readMs": NaN'),
            'Infinity': text.replace('"readMs": 0.1', '"readMs": Infinity'),
            'duplicate': text.replace('"readMs": 0.1', '"readMs": 0.1, "readMs": 0.2'),
            'truncated': text[:-3],
            'empty': '',
            'list': '[]',
        }
        for name, case in cases.items():
            with self.subTest(name):
                self.assertNotEqual(case, text)
                trace, reason = self.check(case)
                self.assertIsNone(trace)
                self.assertEqual(reason, 'missing' if name == 'list' else 'malformed')
        trace, reason = self.check(text.replace('"readMs": 0.1', '"readMs": 1e999'))
        self.assertEqual((trace, reason), (None, 'unhealthy'))

    def test_native_reasons_pass_through_only_when_fixed(self):
        for reason in sorted(comparison.TRACE_NATIVE_REASONS):
            receipt = {'id': 7, 'status': 'ok', 'trace': {'schema': 1, 'available': False, 'reason': reason}}
            self.assertEqual(self.check(receipt), (None, reason))
        for trace in ({'schema': 1, 'available': False, 'reason': 'C:/private/config.json'},
                      {'schema': 1, 'available': False, 'reason': ['admission']},
                      {'schema': 1, 'available': False, 'reason': 'admission', 'error': 'text'},
                      {'schema': 1, 'available': 'no', 'reason': 'admission'}):
            self.assertEqual(self.check({'id': 7, 'status': 'ok', 'trace': trace}), (None, 'malformed'))

    def test_row_keeps_position_when_its_diagnostic_getter_fails(self):
        spans = comparison.DriverSpans(7)
        with patch.object(comparison, 'native_trace', side_effect=RuntimeError('private text')):
            row = comparison.response_trace_row('dashboard', 2, spans)
        self.assertEqual((row['route'], row['cycle'], row['native'], row['nativeUnavailable']),
                         ('dashboard', 2, None, 'assembly'))
        self.assertNotIn('private text', json.dumps(row))


class DriverSpanTests(unittest.TestCase):
    def test_durations_come_from_one_clock_and_cover_each_step(self):
        ticks = iter([1.0, 1.002, 1.003, 1.01, 1.011, 1.02, 1.021, 1.05])
        spans = comparison.DriverSpans(4, clock=lambda: next(ticks))
        spans.mark('start')
        spans.mark('written')
        spans.mark('replaced')
        self.assertEqual(spans.poll(lambda: 'first'), 'first')
        self.assertEqual(spans.poll(lambda: 'second'), 'second')
        spans.mark('received')
        row = spans.driver_row()
        self.assertAlmostEqual(row['writeMs'], 2)
        self.assertAlmostEqual(row['replaceMs'], 1)
        self.assertAlmostEqual(row['waitMs'], 47)
        self.assertEqual(row['pollCount'], 2)
        self.assertAlmostEqual(row['maxReadMs'], 1)
        self.assertAlmostEqual(row['maxPollGapMs'], 9)

    def test_a_poll_read_runs_once_and_its_exact_error_propagates(self):
        calls, failure = [], OSError('sharing violation')
        def read():
            calls.append(1)
            raise failure
        spans = comparison.DriverSpans(4)
        with self.assertRaises(OSError) as raised:
            spans.poll(read)
        self.assertIs(raised.exception, failure)
        self.assertEqual((len(calls), spans.poll_count), (1, 1))

    def test_a_failed_clock_never_stops_the_protocol_call(self):
        def broken():
            raise RuntimeError('clock')
        spans = comparison.DriverSpans(4, clock=broken)
        spans.mark('start')
        self.assertEqual(spans.poll(lambda: 'read'), 'read')
        self.assertIsNone(spans.driver_row())

    def test_invalid_clock_values_do_not_escape_or_qualify_driver_data(self):
        for invalid in (float('nan'), float('inf'), -float('inf'), True, 'private-clock-text', None):
            with self.subTest(invalid=invalid):
                ticks = iter([1.0, 1.001, 1.002, invalid, 1.004, 1.005])
                spans = comparison.DriverSpans(4, clock=lambda: next(ticks))
                for mark in ('start', 'written', 'replaced'):
                    spans.mark(mark)
                calls = []
                self.assertEqual(spans.poll(lambda: calls.append('read') or 'original'), 'original')
                spans.mark('received')
                self.assertEqual(calls, ['read'])
                self.assertIsNone(spans.driver_row())

    def test_decreasing_or_unbounded_clock_spans_remain_unavailable(self):
        for values in ([1.0, 1.001, 1.002, 1.001, 1.004, 1.005],
                       [1.0, 3602.0, 3602.001, 3602.002, 3602.003, 3602.004]):
            with self.subTest(values=values):
                ticks = iter(values)
                spans = comparison.DriverSpans(4, clock=lambda: next(ticks))
                for mark in ('start', 'written', 'replaced'):
                    spans.mark(mark)
                failure = OSError('original read')
                with self.assertRaises(OSError) as raised:
                    spans.poll(lambda: (_ for _ in ()).throw(failure))
                self.assertIs(raised.exception, failure)
                spans.mark('received')
                self.assertIsNone(spans.driver_row())

    def test_untraced_spans_read_no_clock(self):
        with patch.object(comparison.time, 'perf_counter', side_effect=AssertionError('clock read')):
            spans = comparison.UntracedSpans()
            spans.mark('start')
            spans.retain('receipt')
            self.assertEqual(spans.poll(lambda: 'read'), 'read')


class FixtureProbe:
    """Answers the existing file protocol with a real owned child as its process."""
    def __init__(self, trace):
        self.trace = trace
        self.requests = []
        self.stop = threading.Event()
        self.thread = None
        self.child = None
        self.env = None

    def launch(self, args, env, log):
        self.env = env
        self.child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(120)'])
        self.thread = threading.Thread(target=self.serve, args=(Path(env['BUDDY_PERF_DIR']),))
        self.thread.start()
        return comparison.psutil.Process(self.child.pid)

    def serve(self, root):
        (root / 'ready.json').write_text(json.dumps({'ready': True, 'viewport': {'width': 1, 'height': 1, 'scale': 1}}), encoding='utf-8')
        while not self.stop.is_set():
            request = root / 'request.json'
            try:
                value = json.loads(request.read_text(encoding='utf-8'))
                request.unlink()
            except (OSError, ValueError):
                self.stop.wait(.001)
                continue
            self.requests.append(value)
            receipt = {'id': value['id'], 'status': 'ok', 'viewport': {'width': 1, 'height': 1, 'scale': 1}}
            receipt.update(self.trace(value['id'], value['route']))
            (root / 'response.json').write_text(json.dumps(receipt), encoding='utf-8')

    def finish(self):
        self.stop.set()
        if self.thread:
            self.thread.join()
        if self.child:
            self.child.wait()


def run_fixture(trace, *, traced=True, kind='native', cycles=1):
    probe = FixtureProbe(trace)
    real_sleep = time.sleep
    with tempfile.TemporaryDirectory() as output, \
            patch.object(comparison, 'native_trace_enabled', lambda requested: traced and requested == 'native'), \
            patch.object(comparison.time, 'sleep', lambda seconds: real_sleep(min(seconds, .001))):
        options = SimpleNamespace(warmup=0, settle=0, cycles=cycles, electron_development=False,
                                  output=Path(output) / 'comparison.json')
        try:
            return comparison.run_one(kind, Path(sys.executable), probe, options), probe
        finally:
            probe.finish()


def traced_receipts(request_id, route):
    return {'trace': native_trace_receipt(request_id, previous=request_id > 1)['trace']}


class ResponseTraceRunTests(unittest.TestCase):
    def assert_original_samples(self, result, cycles=1):
        samples = result['responseSamples']
        self.assertEqual(len(samples), len(comparison.ROUTES) * (cycles + 1))
        self.assertEqual([(s['route'], s['cycle']) for s in samples],
                         [(route, cycle) for cycle in range(cycles + 1) for route in comparison.ROUTES])
        self.assertTrue(all(set(s) == {'route', 'cycle', 'ms'} for s in samples))
        self.assertEqual({k: result[k] for k in ('responseMedianMs', 'responseMaxMs')}, comparison.response_metrics(samples))

    def test_rows_match_the_original_samples_one_to_one_and_associate_writes(self):
        result, probe = run_fixture(traced_receipts)
        self.assert_original_samples(result)
        rows = result['responseTrace']
        self.assertEqual([(r['route'], r['cycle']) for r in rows],
                         [(s['route'], s['cycle']) for s in result['responseSamples']])
        self.assertTrue(all(r['nativeUnavailable'] is None for r in rows))
        # Two requests precede the measured cycles; nothing extra is sent for diagnostics.
        self.assertEqual([r['id'] for r in probe.requests], list(range(1, len(rows) + 3)))
        self.assertEqual([r['native']['previousId'] for r in rows], list(range(2, len(rows) + 2)))
        self.assertEqual([r['native']['id'] for r in rows], list(range(3, len(rows) + 3)))
        # The final write has no later receipt, so no row can name it.
        self.assertNotIn(len(rows) + 2, [r['native']['previousId'] for r in rows])
        for row in rows:
            self.assertEqual(set(row['driver']), {'writeMs', 'replaceMs', 'waitMs', 'pollCount', 'maxReadMs', 'maxPollGapMs'})
            self.assertGreaterEqual(row['driver']['pollCount'], 1)

    def test_bad_diagnostics_keep_every_sample_and_lose_only_the_trace(self):
        cases = {
            'missing': (lambda i, r: {}, 'missing'),
            'wrong id': (lambda i, r: {'trace': native_trace_receipt(i + 1)['trace']}, 'wrong-id'),
            'unhealthy': (lambda i, r: {'trace': native_trace_receipt(i, readMs=-1)['trace']}, 'unhealthy'),
            'native refusal': (lambda i, r: {'trace': {'schema': 1, 'available': False, 'reason': 'admission'}}, 'admission'),
        }
        for name, (trace, reason) in cases.items():
            with self.subTest(name):
                result, _ = run_fixture(trace)
                self.assert_original_samples(result)
                self.assertEqual([r['nativeUnavailable'] for r in result['responseTrace']],
                                 [reason] * len(result['responseSamples']))
                self.assertTrue(all(r['native'] is None for r in result['responseTrace']))

    def test_a_failing_assembly_cannot_mask_samples_or_stop_cleanup(self):
        with patch.object(comparison, 'native_trace', side_effect=RuntimeError('private')):
            result, probe = run_fixture(traced_receipts)
        self.assert_original_samples(result)
        self.assertEqual({r['nativeUnavailable'] for r in result['responseTrace']}, {'assembly'})
        self.assertFalse(comparison.psutil.pid_exists(probe.child.pid) and
                         comparison.psutil.Process(probe.child.pid).status() != comparison.psutil.STATUS_ZOMBIE)

    def test_a_valid_trace_cannot_hide_a_failed_status(self):
        def failed(request_id, route):
            value = traced_receipts(request_id, route)
            return value | {'status': 'selection failed'} if request_id == 3 else value
        with self.assertRaisesRegex(RuntimeError, 'selection failed'):
            run_fixture(failed)

    def test_a_receipt_for_another_request_still_times_out(self):
        def other(request_id, route):
            value = traced_receipts(request_id, route)
            return value | {'id': 'other'} if request_id == 3 else value
        real = comparison.wait_for
        with patch.object(comparison, 'wait_for', lambda read, process, timeout=60: real(read, process, timeout=.05)):
            with self.assertRaises(TimeoutError):
                run_fixture(other)

    def test_untraced_runs_keep_the_original_result_shape(self):
        result, _ = run_fixture(traced_receipts, traced=False)
        self.assert_original_samples(result)
        self.assertNotIn('responseTrace', result)
        self.assertEqual(set(result), {'startupMs', 'viewport', 'diagnostics', 'responseMedianMs', 'responseMaxMs',
                                       'responseSamples', 'scenarios', 'peak', 'sampleCount', 'memorySamples', 'log',
                                       'measurementSeconds', 'meanHostCpuPercent', 'maxSamplingGapMs'})

    def test_tracing_is_windows_native_only(self):
        real = comparison.native_trace_enabled
        with patch.object(comparison.os, 'name', 'nt'):
            self.assertTrue(real('native'))
            self.assertFalse(real('electron'))
        with patch.object(comparison.os, 'name', 'posix'):
            self.assertFalse(real('native'))


class TraceCannotReachAVerdictTests(unittest.TestCase):
    def run_pair(self):
        native, _ = run_fixture(traced_receipts)
        memory = {'rssBytes': 400 * 1024 ** 2, 'privateBytes': None}
        electron = copy.deepcopy(native)
        electron.pop('responseTrace')
        for run in (native, electron):
            run['scenarios'] = {label: dict(memory) for label in
                                ('dashboard-cold', 'dashboard-warm', 'settings-idle', 'navigation-baseline', 'navigation-cleanup')}
            run['peak'] = dict(memory)
            run['maxSamplingGapMs'] = 150
        return native, electron

    def test_trace_content_changes_no_summary_or_failure(self):
        native, electron = self.run_pair()
        native |= {'startupMs': 1000, 'responseMedianMs': 30, 'responseMaxMs': 600}
        electron |= {'startupMs': 1000, 'responseMedianMs': 30, 'responseMaxMs': 30}
        runs = [{'native': copy.deepcopy(native), 'electron': copy.deepcopy(electron)} for _ in range(5)]
        summary = comparison.summarize(runs)
        failures = comparison.evaluate(summary)
        self.assertEqual(failures, ['responseMaxMs'])
        for replacement in ([], None, 'text', [{'native': {'secondFrameMs': float('nan')}}], [{'driver': {'waitMs': -1}}]):
            with self.subTest(replacement=replacement):
                changed = copy.deepcopy(runs)
                for pair in changed:
                    pair['native']['responseTrace'] = replacement
                changed_summary = comparison.summarize(changed)
                self.assertEqual(changed_summary, summary)
                self.assertEqual(comparison.evaluate(changed_summary), failures)
        self.assertEqual(comparison.response_metrics(native['responseSamples']),
                         comparison.response_metrics([dict(s) for s in native['responseSamples']]))

    def test_no_gate_function_names_the_trace(self):
        tree = ast.parse(Path(comparison.__file__).read_text(encoding='utf-8'))
        for node in tree.body:
            if isinstance(node, ast.FunctionDef) and node.name in ('response_metrics', 'sampling_gap', 'summarize', 'evaluate', 'write_report'):
                self.assertNotIn('responseTrace', ast.unparse(node), node.name)

    def test_each_row_is_assembled_after_its_sample_was_taken(self):
        source = Path(comparison.__file__).read_text(encoding='utf-8')
        loop = source[source.index('before = time.perf_counter()'):source.index('time.sleep(.5)')]
        self.assertLess(loop.index("'ms': (time.perf_counter() - before) * 1000"), loop.index('response_trace_row('))
        self.assertEqual(loop.count('frame(route)'), 1)

    def test_the_per_run_log_line_leaves_the_rows_out(self):
        source = Path(comparison.__file__).read_text(encoding='utf-8')
        self.assertIn("if k not in ('memorySamples', 'responseSamples', 'responseTrace')}", source)

    def test_budget_policy_has_no_trace_input(self):
        policy = (ROOT / 'perf/native-comparison-budgets.json').read_text(encoding='utf-8')
        self.assertNotIn('responseTrace', policy)


REAL_POPEN = subprocess.Popen
SLEEPER = 'import time; time.sleep(120)'
PARENT_OF_SLEEPER = 'import subprocess, sys, time; subprocess.Popen([sys.executable, "-c", "import time; time.sleep(120)"]); time.sleep(120)'
SURVIVED = 'owned processes survived cleanup'


def gone(process):
    try:
        return not process.is_running() or process.status() == comparison.psutil.STATUS_ZOMBIE
    except comparison.psutil.NoSuchProcess:
        return True


class CleanupProbe(FixtureProbe):
    """The file protocol with a real owned child. It stops answering after `answer_until`,
    keeps the log run_one gave it, and its finish kills only its own still-running processes,
    so a bypassed terminate shows up as `alive_at_finish` instead of a hang."""
    def __init__(self, answer_until=None, grandchild=False, wait_fails=False):
        super().__init__(lambda request_id, route: {} if answer_until is None or request_id <= answer_until
                         else {'id': 'silent'})
        self.command = [sys.executable, '-c', PARENT_OF_SLEEPER if grandchild else SLEEPER]
        self.grandchild, self.wait_fails = grandchild, wait_fails
        self.log = self.grandchild_process = self.alive_at_finish = None
        self.child_waits = []

    def launch(self, args, env, log):
        self.log = log
        return self.start(env, REAL_POPEN(self.command))

    def popen(self, args, **kwargs):
        """Stands in for subprocess.Popen on the no-desktop path with the same cwd, env and log."""
        self.log = kwargs['stdout']
        child = REAL_POPEN(self.command, **kwargs)
        real_wait = child.wait
        def wait(timeout=None):
            self.child_waits.append(timeout)
            code = real_wait(timeout=timeout)
            if self.wait_fails:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return code
        child.wait = wait
        return self.start(kwargs['env'], child, keep=child)

    def start(self, env, child, keep=None):
        self.child = child
        if self.grandchild:
            deadline = time.monotonic() + 10
            while not (found := comparison.psutil.Process(child.pid).children()) and time.monotonic() < deadline:
                time.sleep(.01)
            self.grandchild_process = found[0]
        self.env = env
        self.thread = threading.Thread(target=self.serve, args=(Path(env['BUDDY_PERF_DIR']),))
        self.thread.start()
        return keep if keep is not None else comparison.psutil.Process(child.pid)

    def finish(self):
        self.stop.set()
        if self.thread:
            self.thread.join()
        if self.child:
            self.alive_at_finish = self.child.poll() is None
            if self.alive_at_finish:
                self.child.kill()
            REAL_POPEN.wait(self.child, timeout=10)
        if self.grandchild_process and not gone(self.grandchild_process):
            self.alive_at_finish = True
            self.grandchild_process.kill()


class ClosingFails:
    """The run's real log file. close really closes it, then reports a failure."""
    def __init__(self, file):
        self.file = file

    def fileno(self):
        return self.file.fileno()

    @property
    def closed(self):
        return self.file.closed

    def close(self):
        self.file.close()
        raise OSError('log close failed')


class FakeCDP:
    """Electron's transport, answering every check until frame call `fail_at` reports a missing shell."""
    instances = []

    def __init__(self, port, process, fail_at=3, close_fails=True):
        self.frames, self.closes, self.fail_at, self.close_fails = 0, 0, fail_at, close_fails
        self.ws = SimpleNamespace(close=self.close)
        FakeCDP.instances.append(self)

    def evaluate(self, expression):
        return {'width': 1, 'height': 1, 'scale': 1} if 'innerWidth' in expression else True

    def frame(self, route):
        self.frames += 1
        return self.frames != self.fail_at

    def close_tabs(self):
        pass

    def close(self):
        self.closes += 1
        if self.close_fails:
            raise OSError('socket already closed')


def run_cleanup(probe, *, kind='native', desktop=True, survivor=False, cdp=None, log_close_fails=False,
                profile_fails=False, interrupt=False):
    """Run the real run_one. Fakes are named: a survivor keeps psutil's real kill and real wait but
    reports the root process as still alive; every other process step is real."""
    real_sleep, real_wait_for = time.sleep, comparison.wait_for
    real_wait_procs, real_terminate, real_open = comparison.psutil.wait_procs, comparison.terminate, Path.open
    real_profile = tempfile.TemporaryDirectory
    record = SimpleNamespace(wait_errors=[], waited=[], terminated=0, result=None, error=None)

    def wait_for(read, process, timeout=60):
        try:
            return real_wait_for(read, process, timeout=.5)
        except BaseException as error:
            record.wait_errors.append(error)
            raise

    def wait_procs(procs, timeout=None):
        record.waited.append(([p.pid for p in procs], timeout))
        done, alive = real_wait_procs(procs, timeout=timeout)
        return (done, alive + [procs[-1]]) if survivor else (done, alive)

    def terminate(process):
        record.terminated += 1
        return real_terminate(process)

    def opener(path, mode='r', *args, **kwargs):
        file = real_open(path, mode, *args, **kwargs)
        return ClosingFails(file) if log_close_fails and mode == 'wb' and path.suffix == '.log' else file

    class FailingProfile(real_profile):
        def __exit__(self, *exc):
            super().__exit__(*exc)
            raise OSError('profile removal failed')

    def warmup(seconds, kind):
        if interrupt:
            raise KeyboardInterrupt('operator stop')

    patches = [patch.object(comparison, 'native_trace_enabled', lambda requested: False),
               patch.object(comparison.time, 'sleep', lambda seconds: real_sleep(min(seconds, .001))),
               patch.object(comparison, 'wait_for', wait_for), patch.object(comparison, 'terminate', terminate),
               patch.object(comparison.psutil, 'wait_procs', wait_procs), patch.object(comparison.Path, 'open', opener),
               patch.object(comparison, 'warmup', warmup)]
    if cdp:
        patches.append(patch.object(comparison, 'CDP', cdp))
    if not desktop:
        patches.append(patch.object(comparison.subprocess, 'Popen', probe.popen))
    if profile_fails:
        patches.append(patch.object(comparison.tempfile, 'TemporaryDirectory', FailingProfile))
    with tempfile.TemporaryDirectory() as output:
        options = SimpleNamespace(warmup=0, settle=0, cycles=1, electron_development=False,
                                  output=Path(output) / 'comparison.json')
        try:
            for item in patches:
                item.start()
            record.result = comparison.run_one(kind, Path(sys.executable), probe if desktop else None, options)
        except BaseException as error:
            record.error = error
        finally:
            for item in reversed(patches):
                item.stop()
            probe.finish()
    return record


def entry(step, failure_type, message, raised=False):
    return {'step': step, 'type': failure_type, 'message': message, 'raised': raised}


class CleanupPreservationTests(unittest.TestCase):
    """The e343 Intel boundary: a navigation receipt timed out, then cleanup found a survivor."""
    def assert_only_requests(self, probe, count):
        self.assertEqual([r['id'] for r in probe.requests], list(range(1, count + 1)))
        expected = ['dashboard', 'settings-accounts'] + list(comparison.ROUTES) * 2
        self.assertEqual([r['route'] for r in probe.requests], expected[:count])

    def assert_cleaned(self, probe, record, *, desktop=True):
        self.assertEqual(record.terminated, 1)
        self.assertEqual(len(record.waited), 1)
        self.assertEqual(record.waited[0][1], 10)
        self.assertIn(probe.child.pid, record.waited[0][0])
        self.assertTrue(probe.log.closed)
        self.assertFalse(probe.alive_at_finish)
        if not desktop:
            self.assertEqual(probe.child_waits, [10])

    def test_e343_missing_receipt_stays_the_error_when_cleanup_finds_a_survivor(self):
        """Faked survivor: the kill and the 10 s wait are real, only the liveness answer is not."""
        for desktop in (True, False):
            with self.subTest(desktop=desktop):
                probe = CleanupProbe(answer_until=3)
                record = run_cleanup(probe, desktop=desktop, survivor=True)
                self.assertIsNone(record.result)
                self.assertEqual(len(record.wait_errors), 1)
                self.assertIs(record.error, record.wait_errors[0])
                self.assertIsInstance(record.error, TimeoutError)
                self.assertEqual(str(record.error), 'missing application receipt')
                self.assertEqual(record.error.cleanup_failures, [entry('terminate', 'RuntimeError', SURVIVED)])
                self.assertEqual(record.error.__notes__, [f'cleanup step terminate also failed: RuntimeError: {SURVIVED}'])
                # The fourth request is cycle 0's second route; nothing is sent after it.
                self.assert_only_requests(probe, 4)
                self.assertEqual(probe.requests[3]['route'], comparison.ROUTES[1])
                self.assert_cleaned(probe, record, desktop=desktop)

    def test_cleanup_failure_alone_still_fails_and_finishes_every_later_step(self):
        probe = CleanupProbe()
        record = run_cleanup(probe, desktop=False, survivor=True)
        self.assertIsNone(record.result)
        self.assertEqual(record.wait_errors, [])
        self.assertIsInstance(record.error, RuntimeError)
        self.assertEqual(str(record.error), SURVIVED)
        self.assertEqual(record.error.cleanup_failures, [entry('terminate', 'RuntimeError', SURVIVED, raised=True)])
        self.assertFalse(hasattr(record.error, '__notes__'))
        self.assert_only_requests(probe, 2 + 2 * len(comparison.ROUTES))
        self.assert_cleaned(probe, record, desktop=False)

    def test_every_cleanup_failure_is_listed_in_order_on_the_primary(self):
        """Faked: survivor answer, log close error after a real close, child wait error after a real reap."""
        probe = CleanupProbe(answer_until=3, wait_fails=True)
        record = run_cleanup(probe, desktop=False, survivor=True, log_close_fails=True, profile_fails=True)
        self.assertIs(record.error, record.wait_errors[0])
        self.assertEqual([(f['step'], f['type'], f['raised']) for f in record.error.cleanup_failures],
                         [('terminate', 'RuntimeError', False), ('log', 'OSError', False),
                          ('child', 'TimeoutExpired', False), ('profile', 'OSError', False)])
        self.assertEqual(len(record.error.__notes__), 4)
        self.assert_cleaned(probe, record, desktop=False)

    def test_a_cleanup_only_failure_keeps_later_failures_on_itself(self):
        probe = CleanupProbe()
        record = run_cleanup(probe, desktop=False, survivor=True, log_close_fails=True, profile_fails=True)
        self.assertEqual(str(record.error), SURVIVED)
        self.assertEqual(record.error.cleanup_failures, [entry('terminate', 'RuntimeError', SURVIVED, raised=True),
                                                         entry('log', 'OSError', 'log close failed'),
                                                         entry('profile', 'OSError', 'profile removal failed')])
        self.assert_cleaned(probe, record, desktop=False)

    def test_profile_removal_failure_cannot_replace_the_primary(self):
        probe = CleanupProbe(answer_until=3)
        record = run_cleanup(probe, profile_fails=True)
        self.assertIs(record.error, record.wait_errors[0])
        self.assertEqual(record.error.cleanup_failures, [entry('profile', 'OSError', 'profile removal failed')])
        self.assert_cleaned(probe, record)
        probe = CleanupProbe()
        record = run_cleanup(probe, profile_fails=True)
        self.assertEqual((type(record.error), str(record.error)), (OSError, 'profile removal failed'))
        self.assertIsNone(record.result)

    def test_a_failed_transport_close_cannot_skip_terminate(self):
        """Real termination: the fake is only Electron's transport; kill and wait are psutil's own."""
        FakeCDP.instances.clear()
        probe = CleanupProbe()
        record = run_cleanup(probe, kind='electron', cdp=FakeCDP)
        self.assertEqual((type(record.error), str(record.error)), (RuntimeError, 'Electron shell missing'))
        self.assertEqual(record.error.cleanup_failures, [entry('cdp', 'OSError', 'socket already closed')])
        [cdp] = FakeCDP.instances
        self.assertEqual((cdp.frames, cdp.closes), (3, 1))
        self.assertEqual(probe.requests, [])
        self.assert_cleaned(probe, record)

    def test_an_interrupt_stays_the_error_and_cleanup_still_runs(self):
        probe = CleanupProbe()
        record = run_cleanup(probe, survivor=True, interrupt=True)
        self.assertIsInstance(record.error, KeyboardInterrupt)
        self.assertEqual(record.error.cleanup_failures, [entry('terminate', 'RuntimeError', SURVIVED)])
        self.assert_only_requests(probe, 1)
        self.assert_cleaned(probe, record)

    def test_real_owned_tree_is_killed_and_an_unrelated_process_is_left_alone(self):
        """Real process proof, nothing faked in cleanup: an owned child with a real grandchild,
        plus a sibling the run does not own."""
        sibling = REAL_POPEN([sys.executable, '-c', SLEEPER])
        try:
            probe = CleanupProbe(answer_until=3, grandchild=True)
            record = run_cleanup(probe, desktop=False)
            self.assertIs(record.error, record.wait_errors[0])
            self.assertFalse(hasattr(record.error, 'cleanup_failures'))
            self.assertIn(probe.grandchild_process.pid, record.waited[0][0])
            self.assertTrue(gone(probe.grandchild_process))
            self.assert_cleaned(probe, record, desktop=False)
            self.assertIsNone(sibling.poll())
            self.assertNotIn(sibling.pid, record.waited[0][0])
        finally:
            sibling.kill()
            sibling.wait(timeout=10)

    def test_a_successful_run_returns_its_result_with_no_cleanup_record(self):
        probe = CleanupProbe()
        record = run_cleanup(probe, desktop=False)
        self.assertIsNone(record.error)
        self.assertEqual(len(record.result['responseSamples']), 2 * len(comparison.ROUTES))
        self.assert_cleaned(probe, record, desktop=False)


class ErrorFileTests(unittest.TestCase):
    """main's error file is where the evidence goes. It names the primary and lists cleanup failures."""
    def write_error(self, error):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root) / 'performance/comparison.json'
            argv = ['native-comparison.py', '--native', sys.executable, '--electron', sys.executable,
                    '--electron-development', '--output', str(output)]
            with patch.object(sys, 'argv', argv), patch.dict(comparison.os.environ, {'GITHUB_ACTIONS': 'true'}), \
                    patch.object(comparison.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), \
                    patch.object(comparison, 'HiddenDesktop', Mock()), \
                    patch.object(comparison, 'run_one', Mock(side_effect=error)), \
                    patch('builtins.print'):
                with self.assertRaises(BaseException) as raised:
                    comparison.main()
            self.assertIs(raised.exception, error)
            return json.loads(output.read_text(encoding='utf-8'))

    def test_the_primary_and_its_cleanup_failure_both_reach_the_file(self):
        error = TimeoutError('missing application receipt')
        def survivor():
            raise RuntimeError(SURVIVED)
        comparison.finish_run(error, [('terminate', survivor)])
        report = self.write_error(error)
        self.assertEqual(report['error'], 'missing application receipt')
        self.assertEqual(report['cleanupFailures'], [entry('terminate', 'RuntimeError', SURVIVED)])
        self.assertEqual((report['status'], report['runtime'], report['completedPairs'], report['partialPair']),
                         ('error', 'native', [], {}))

    def test_an_error_without_cleanup_failures_keeps_the_original_file_shape(self):
        report = self.write_error(RuntimeError('Electron shell missing'))
        self.assertEqual(list(report), ['schema', 'status', 'runtime', 'error', 'completedPairs', 'partialPair'])


class ParentBoundaryTests(unittest.TestCase):
    """No Windows UI: main uses an owned fake desktop and never launches an app."""
    def main_error(self, primary, *, desktop_error=None, evidence_error=None, input_error=None):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root) / 'performance/comparison.json'
            argv = ['native-comparison.py', '--native', sys.executable, '--electron', sys.executable,
                    '--electron-development', '--output', str(output)]
            close = Mock(side_effect=desktop_error)
            run = Mock(side_effect=primary) if primary is not None else Mock(return_value={'sampleCount': 1})
            real_write, writes = Path.write_text, []
            def write(path, *args, **kwargs):
                if path.parent.name == 'inputs' and input_error is not None:
                    raise input_error
                if path == output:
                    writes.append(path)
                    if evidence_error is not None:
                        raise evidence_error
                return real_write(path, *args, **kwargs)
            # Replace this module's os reference, not pathlib's platform selection.
            environment = dict(comparison.os.environ, GITHUB_ACTIONS='true')
            with patch.object(sys, 'argv', argv), \
                    patch.object(comparison, 'os', SimpleNamespace(name='nt', environ=environment)), \
                    patch.object(comparison.subprocess, 'run', return_value=SimpleNamespace(returncode=0)), \
                    patch.object(comparison, 'HiddenDesktop', Mock(return_value=SimpleNamespace(close=close))), \
                    patch.object(comparison, 'run_one', run), patch.object(Path, 'write_text', write), \
                    patch('builtins.print'):
                with self.assertRaises(BaseException) as raised:
                    comparison.main()
            payload = json.loads(output.read_text(encoding='utf-8')) if output.exists() else None
            return raised.exception, payload, close.call_count, run.call_count, len(writes)

    def test_desktop_close_cannot_replace_the_primary_after_error_evidence(self):
        primary, cleanup = TimeoutError('missing application receipt'), OSError('desktop close failed')
        error, payload, closes, runs, writes = self.main_error(primary, desktop_error=cleanup)
        self.assertIs(error, primary)
        self.assertEqual(payload['error'], str(primary))
        self.assertEqual((closes, runs, writes), (1, 1, 1))
        self.assertEqual(error.cleanup_failures, [entry('desktop', 'OSError', str(cleanup))])
        self.assertIn('desktop close failed', error.__notes__[0])

    def test_desktop_cleanup_failure_alone_is_fatal(self):
        cleanup = OSError('desktop close failed')
        error, payload, closes, runs, writes = self.main_error(None, desktop_error=cleanup)
        self.assertIs(error, cleanup)
        self.assertIsNone(payload)
        self.assertEqual((closes, runs, writes), (1, 10, 0))

    def test_error_evidence_write_failure_cannot_replace_the_primary(self):
        primary, write_error = TimeoutError('missing application receipt'), OSError('evidence unwritable')
        error, payload, closes, runs, writes = self.main_error(primary, evidence_error=write_error)
        self.assertIs(error, primary)
        self.assertIsNone(payload)
        self.assertEqual((closes, runs, writes), (1, 1, 1))
        self.assertEqual(error.cleanup_failures, [entry('evidence', 'OSError', str(write_error))])

    def test_input_copy_failure_still_closes_the_owned_desktop(self):
        primary = OSError('input evidence unreadable')
        error, payload, closes, runs, writes = self.main_error(None, input_error=primary)
        self.assertIs(error, primary)
        self.assertIsNone(payload)
        self.assertEqual((closes, runs, writes), (1, 0, 0))

    def test_cleanup_interrupt_preserves_the_primary_and_later_cleanup(self):
        primary, interrupt, calls = TimeoutError('missing application receipt'), KeyboardInterrupt('cleanup interrupted'), []
        def interrupted():
            raise interrupt
        comparison.finish_run(primary, [('interrupt', interrupted), ('later', lambda: calls.append('later'))])
        self.assertEqual(calls, ['later'])
        self.assertEqual(primary.cleanup_failures, [entry('interrupt', 'KeyboardInterrupt', str(interrupt))])
        with self.assertRaises(KeyboardInterrupt) as raised:
            comparison.finish_run(None, [('interrupt', interrupted)])
        self.assertIs(raised.exception, interrupt)

    def test_main_interrupt_survives_desktop_cleanup_error(self):
        primary = KeyboardInterrupt('operator stop')
        error, payload, closes, runs, writes = self.main_error(primary, desktop_error=OSError('desktop close failed'))
        self.assertIs(error, primary)
        self.assertIsNone(payload)
        self.assertEqual((closes, runs, writes), (1, 1, 0))


class ProbeSourceContractTests(unittest.TestCase):
    probe = (ROOT / 'rust/crates/buddy-app/src/performance_probe.rs').read_text(encoding='utf-8')
    trace = (ROOT / 'rust/crates/buddy-app/src/performance_probe/trace.rs').read_text(encoding='utf-8')
    settings = (ROOT / 'rust/crates/buddy-app/src/settings.rs').read_text(encoding='utf-8')

    @staticmethod
    def product(source):
        return source.split('#[cfg(test)]\nmod tests', 1)[0]

    def test_no_new_callback_task_thread_or_frame_request(self):
        probe, trace = self.product(self.probe), self.product(self.trace)
        self.assertEqual(probe.count('on_next_frame('), 2)
        self.assertEqual(probe.count('window.refresh()'), 2)
        self.assertEqual(probe.count('.spawn('), 1)
        self.assertEqual(probe.count('.detach()'), 1)
        for forbidden in ('spawn', 'on_next_frame', 'refresh', 'timer', 'sleep', 'thread::Builder', 'std::fs', 'unwrap()', 'expect(', 'panic!', 'unsafe'):
            self.assertFalse(forbidden in trace, forbidden)

    def test_native_field_names_match_the_driver_contract(self):
        probe = self.product(self.probe)
        for key in comparison.TRACE_KEYS:
            self.assertIn(f'"{key}"', probe, key)
        for stage in comparison.TRACE_STAGES:
            self.assertIn(f'"{stage}"', probe, stage)
        trace = self.product(self.trace)
        for reason in comparison.TRACE_NATIVE_REASONS:
            self.assertIn(f'"{reason}"', trace, reason)

    def test_settings_wraps_each_stage_at_its_one_original_call(self):
        settings = self.product(self.settings)
        for stage, call in (('Lock', 'ConfigLock::acquire(path, DEFAULT_TIMEOUT)'),
                            ('Load', 'AppConfig::load_with_stamp_from(path)'),
                            ('Save', 'fresh.save_if_unchanged_at(path, &stamp)'),
                            ('Install', 'Self::install(cx, fresh)')):
            self.assertEqual(settings.count(f'trace::measure(Stage::{stage},'), 1, stage)
            self.assertEqual(settings.count(call), 1, call)
        self.assertEqual(settings.count('trace::attempt()'), 1)
        self.assertNotIn('StageScope', settings)

    def test_only_the_probe_arms_a_scope_and_only_around_navigation(self):
        probe = self.product(self.probe)
        self.assertEqual(probe.count('arm_settings()'), 1)
        armed = probe[probe.index('arm_settings()'):probe.index('spans.settings_finished(stages)')]
        self.assertEqual(armed.count('app.set_section(section, cx);'), 1)
        self.assertEqual(armed.count(';'), 2)


if __name__ == '__main__':
    unittest.main()
