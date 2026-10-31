"""Paired empty-account performance qualification. Requires psutil and websocket-client.

Windows launches on a never-shown desktop. Unix runs require CI. No user input,
credential writes, application defaults, or installed application state is changed.
"""
import argparse
import ctypes
from ctypes import wintypes as w
import hashlib
from datetime import datetime
import json
import math
import os
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import threading
import time
import urllib.request
import tempfile

import psutil
import websocket

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = json.loads((ROOT / 'perf/matching-fixture.json').read_text(encoding='utf-8'))
ROUTES = CONTRACT['routes']
FIXTURE = CONTRACT['config']


class HiddenDesktop:
    """CreateProcess desktop isolation, without switching the user's desktop."""
    def __init__(self):
        self.name = 'buddy-perf-' + str(os.getpid())
        self.user = ctypes.WinDLL('user32', use_last_error=True)
        self.kernel = ctypes.WinDLL('kernel32', use_last_error=True)
        self.user.CreateDesktopW.argtypes = [w.LPCWSTR, w.LPCWSTR, ctypes.c_void_p, w.DWORD, w.DWORD, ctypes.c_void_p]
        self.user.CreateDesktopW.restype = w.HANDLE
        self.user.CloseDesktop.argtypes = [w.HANDLE]
        self.kernel.CloseHandle.argtypes = [w.HANDLE]
        self.handle = self.user.CreateDesktopW(self.name, None, None, 0, 0x01ff, None)
        if not self.handle:
            raise ctypes.WinError(ctypes.get_last_error())

    def launch(self, args, env, log):
        class SI(ctypes.Structure):
            _fields_ = [('cb', w.DWORD), ('reserved', w.LPWSTR), ('desktop', w.LPWSTR),
                        ('title', w.LPWSTR), ('x', w.DWORD), ('y', w.DWORD),
                        ('xs', w.DWORD), ('ys', w.DWORD), ('xc', w.DWORD),
                        ('yc', w.DWORD), ('fill', w.DWORD), ('flags', w.DWORD),
                        ('show', w.WORD), ('reserved2size', w.WORD),
                        ('reserved2', ctypes.c_void_p), ('stdin', w.HANDLE),
                        ('stdout', w.HANDLE), ('stderr', w.HANDLE)]
        class PI(ctypes.Structure):
            _fields_ = [('process', w.HANDLE), ('thread', w.HANDLE), ('pid', w.DWORD), ('tid', w.DWORD)]
        si, pi = SI(), PI()
        si.cb, si.desktop = ctypes.sizeof(SI), self.name
        import msvcrt
        os.set_inheritable(log.fileno(), True)
        si.flags = 0x100
        si.stdout = si.stderr = msvcrt.get_osfhandle(log.fileno())
        si.stdin = None
        block = ctypes.create_unicode_buffer('\0'.join(f'{k}={v}' for k, v in sorted(env.items())) + '\0\0')
        fn = self.kernel.CreateProcessW
        fn.argtypes = [w.LPCWSTR, w.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, w.BOOL,
                       w.DWORD, ctypes.c_void_p, w.LPCWSTR, ctypes.POINTER(SI), ctypes.POINTER(PI)]
        fn.restype = w.BOOL
        if not fn(args[0], ctypes.create_unicode_buffer(subprocess.list2cmdline(args)),
                  None, None, True, 0x08000400, block, str(ROOT), ctypes.byref(si), ctypes.byref(pi)):
            raise ctypes.WinError(ctypes.get_last_error())
        self.kernel.CloseHandle(pi.thread)
        self.kernel.CloseHandle(pi.process)
        return psutil.Process(pi.pid)

    def close(self):
        if not self.user.CloseDesktop(self.handle):
            raise ctypes.WinError(ctypes.get_last_error())


def wait_for(read, process, timeout=60):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process.is_running():
            raise RuntimeError('application exited before receipt')
        try:
            value = read()
            if value:
                return value
        except (OSError, ValueError, websocket.WebSocketException):
            pass
        time.sleep(.005)
    raise TimeoutError('missing application receipt')


class CDP:
    def __init__(self, port, process):
        def target():
            with urllib.request.urlopen(f'http://127.0.0.1:{port}/json/list', timeout=1) as response:
                return next((x for x in json.load(response) if x['type'] == 'page'), None)
        target = wait_for(target, process)
        self.ws = websocket.create_connection(target['webSocketDebuggerUrl'], timeout=60, suppress_origin=True)
        self.seq = 0

    def evaluate(self, expression):
        self.seq += 1
        self.ws.send(json.dumps({'id': self.seq, 'method': 'Runtime.evaluate',
                                'params': {'expression': expression, 'awaitPromise': True, 'returnByValue': True}}))
        while True:
            value = json.loads(self.ws.recv())
            if value.get('id') != self.seq:
                continue
            if 'error' in value or 'exceptionDetails' in value.get('result', {}):
                raise RuntimeError(value)
            return value['result']['result'].get('value')

    def close_tabs(self):
        self.evaluate("[...document.querySelectorAll('.tab-select')].find(x => x.textContent.trim() === 'Dashboard')?.click()")
        for _ in range(100):
            closed = self.evaluate("(() => { const b = document.querySelector('.tab-close:not([aria-label=\"Close Dashboard\"])'); if (b) b.click(); return Boolean(b); })()")
            if not closed:
                return
            time.sleep(.1)
        raise RuntimeError('Electron tabs did not close')

    def frame(self, route=None):
        navigate = '' if route is None else f"window.dispatchEvent(new CustomEvent('app:navigate', {{detail: {{viewId: {json.dumps(route)}}}}}));"
        expected = json.dumps(CONTRACT['electronLabels'][route or 'dashboard'])
        check = f"document.querySelector('.tab-select[aria-selected=\"true\"] .tab-label')?.textContent?.trim() === {expected}"
        return self.evaluate("new Promise(resolve => {" + navigate +
                             f"requestAnimationFrame(() => requestAnimationFrame(() => resolve({check})));" + "})")


def memory(process):
    total, private = 0, 0 if os.name == 'nt' else None
    processes = [process] + process.children(recursive=True)
    for child in processes:
        try:
            info = child.memory_info()
            total += info.rss
            if private is not None:
                private += info.private
        except psutil.NoSuchProcess:
            continue
    if total <= 0:
        raise RuntimeError('empty process tree memory sample')
    return {'rssBytes': total, 'privateBytes': private}


def terminate(process):
    children = process.children(recursive=True) if process.is_running() else []
    for child in reversed(children + [process]):
        try:
            child.kill()
        except psutil.NoSuchProcess:
            pass
    _, alive = psutil.wait_procs(children + [process], timeout=10)
    if alive:
        raise RuntimeError('owned processes survived cleanup')


def run_one(kind, exe, desktop, options):
    with tempfile.TemporaryDirectory(prefix='buddy-paired-perf-') as temp:
        profile = Path(temp)
        (profile / 'config.json').write_text(json.dumps(FIXTURE), encoding='utf-8')
        (profile / 'window-state.json').write_text(json.dumps(CONTRACT['windowState']), encoding='utf-8')
        env = {k: v for k, v in os.environ.items() if not k.startswith(('BUDDY_', 'OTEL_'))}
        env.update(BUDDY_CONFIG_PATH=str(profile / 'config.json'), BUDDY_CONVEX_URL='http://127.0.0.1:9',
                   BUDDY_PERF_DIR=temp, GPUI_DISABLE_DIRECT_COMPOSITION='1')
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        args = [str(exe)]
        if kind == 'electron':
            env.update(BUDDY_DEBUG_PORT=str(port), BUDDY_MEMORY_HARNESS='1')
            if options.electron_development:
                args += [str(ROOT)]
            args += [f'--user-data-dir={temp}']
        started = time.perf_counter()
        child = None
        log_path = options.output.parent / f'{kind}-{time.time_ns()}.log'
        log = log_path.open('wb')
        if desktop:
            process = desktop.launch(args, env, log)
        else:
            child = subprocess.Popen(args, cwd=ROOT, env=env, stdout=log, stderr=log)
            process = psutil.Process(child.pid)
        stop, samples, errors = threading.Event(), [], []
        def sample():
            while not stop.is_set():
                try:
                    samples.append(memory(process) | {'atSeconds': time.perf_counter() - started})
                except Exception as error:
                    errors.append(str(error))
                    break
                stop.wait(.1)
        thread = threading.Thread(target=sample)
        thread.start()
        cdp = None
        seq = 0
        def frame(route):
            nonlocal seq
            if kind == 'electron':
                if not cdp.frame(route):
                    raise RuntimeError('Electron shell missing')
            else:
                seq += 1
                request = profile / 'request.tmp'
                request.write_text(json.dumps({'id': seq, 'route': route}), encoding='utf-8')
                request.replace(profile / 'request.json')
                def matching_receipt():
                    value = json.loads((profile / 'response.json').read_text(encoding='utf-8'))
                    return value if value.get('id') == seq else None
                receipt = wait_for(matching_receipt, process)
                if receipt['status'] != 'ok':
                    raise RuntimeError(receipt)
        try:
            if kind == 'native':
                ready = wait_for(lambda: json.loads((profile / 'ready.json').read_text(encoding='utf-8')), process)
                frame('dashboard')
            else:
                cdp = CDP(port, process)
                wait_for(lambda: cdp.evaluate("Boolean(document.querySelector('.activity-bar'))"), process)
                if not cdp.frame('dashboard'):
                    raise RuntimeError('Electron Dashboard frame missing')
            startup = (time.perf_counter() - started) * 1000
            viewport = ready['viewport'] if kind == 'native' else cdp.evaluate(
                '({width: innerWidth, height: innerHeight, scale: devicePixelRatio})')
            scenarios = {'dashboard-cold': memory(process)}
            time.sleep(options.warmup)
            scenarios['dashboard-warm'] = memory(process)
            frame('settings-accounts')
            time.sleep(options.settle)
            scenarios['settings-idle'] = memory(process)
            latencies = []
            for cycle in range(options.cycles + 1):
                for route in ROUTES:
                    before = time.perf_counter()
                    frame(route)
                    latencies.append((time.perf_counter() - before) * 1000)
                    time.sleep(.5)
                if cdp:
                    cdp.close_tabs()
                time.sleep(options.settle)
                scenarios['navigation-baseline' if cycle == 0 else 'navigation-cleanup'] = memory(process)
            if errors:
                raise RuntimeError(errors)
            diagnostic_text = log_path.read_text(encoding='utf-8', errors='replace')
            diagnostics = {message: diagnostic_text.count(message) for message in
                           ('RefCell already borrowed', 'unable to get cursor position')}
            if diagnostics['RefCell already borrowed']:
                raise RuntimeError('native startup borrow regression; see retained log')
            return {'startupMs': startup, 'viewport': viewport, 'diagnostics': diagnostics, 'responseMedianMs': statistics.median(latencies),
                    'responseMaxMs': max(latencies), 'scenarios': scenarios,
                    'peak': {key: max(s[key] for s in samples + list(scenarios.values())) if samples[0][key] is not None else None
                             for key in ('rssBytes', 'privateBytes')}, 'sampleCount': len(samples), 'memorySamples': samples, 'log': log_path.name,
                    'maxSamplingGapMs': max((b['atSeconds'] - a['atSeconds']) * 1000
                                            for a, b in zip(samples, samples[1:])) if len(samples) > 1 else None}
        finally:
            stop.set()
            thread.join()
            if cdp:
                cdp.ws.close()
            terminate(process)
            log.close()
            if child:
                child.wait(timeout=10)


def summarize(runs):
    result = {}
    for kind in ('native', 'electron'):
        values = [r[kind] for r in runs]
        result[kind] = {key: statistics.median(v[key] for v in values)
                        for key in ('startupMs', 'responseMedianMs', 'responseMaxMs')}
        for label in ('dashboard-cold', 'dashboard-warm', 'settings-idle', 'navigation-baseline', 'navigation-cleanup', 'peak'):
            metrics = [v['peak'] if label == 'peak' else v['scenarios'][label] for v in values]
            result[kind][label] = {key: None if metrics[0][key] is None else statistics.median(m[key] for m in metrics)
                                   for key in ('rssBytes', 'privateBytes')}
    return result


def evaluate(summary):
    budgets = json.loads((ROOT / 'perf/native-comparison-budgets.json').read_text(encoding='utf-8'))
    failures = []
    for key, budget in budgets['timing'].items():
        native, electron = summary['native'][key], summary['electron'][key]
        if not all(isinstance(v, (int, float)) and math.isfinite(v) and v > 0 for v in (native, electron)):
            raise ValueError('invalid timing sample')
        if native > max(budget['floor'], electron * budget['ratio']):
            failures.append(key)
    for label in budgets['memory']['scenarios']:
        for key in ('rssBytes', 'privateBytes'):
            native, electron = summary['native'][label][key], summary['electron'][label][key]
            if key == 'privateBytes' and native is None and electron is None:
                continue
            if not all(isinstance(v, (int, float)) and math.isfinite(v) and v > 0 for v in (native, electron)):
                raise ValueError('invalid process memory sample')
            if native > max(budgets['memory']['floorBytes'], electron * budgets['memory']['ratio']):
                failures.append(f'{label}.{key}')
    growth = budgets['cleanupGrowth']
    for key in ('rssBytes', 'privateBytes'):
        baseline = summary['native'][growth['baseline']][key]
        cleanup = summary['native'][growth['scenario']][key]
        if key == 'privateBytes' and baseline is None and cleanup is None:
            continue
        if not all(isinstance(v, (int, float)) and math.isfinite(v) and v > 0 for v in (baseline, cleanup)):
            raise ValueError('invalid navigation growth sample')
        if cleanup > max(baseline * growth['ratio'], baseline + growth['headroomBytes']):
            failures.append(f'navigation-growth.{key}')
    return failures


def write_report(output, summary):
    rows = []
    for key, label in [('startupMs', 'Startup to frame receipt'), ('responseMedianMs', 'View switch median'), ('responseMaxMs', 'View switch maximum')]:
        rows.append((label, summary['native'][key], summary['electron'][key], 'ms'))
    for label in ('dashboard-cold', 'dashboard-warm', 'settings-idle', 'navigation-baseline', 'navigation-cleanup', 'peak'):
        for key in ('rssBytes', 'privateBytes'):
            a, b = summary['native'][label][key], summary['electron'][label][key]
            if a is not None:
                rows.append((f'{label} {key}', a / 1024**2, b / 1024**2, 'MiB'))
    text = '| Metric | Native | Electron | Unit |\n| --- | ---: | ---: | --- |\n'
    text += ''.join(f'| {label} | {a:.2f} | {b:.2f} | {unit} |\n' for label, a, b, unit in rows)
    output.with_suffix('.md').write_text(text, encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=Path, required=True)
    parser.add_argument('--electron', type=Path, required=True)
    parser.add_argument('--electron-development', action='store_true')
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--warmup', type=float, default=300)
    parser.add_argument('--settle', type=float, default=5)
    parser.add_argument('--cycles', type=int, default=3)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if not args.native.is_file() or not args.electron.is_file():
        parser.error('both executables must exist')
    if args.runs < 5 or args.warmup < 0 or args.settle < 0 or args.cycles < 1:
        parser.error('require >=5 runs, nonnegative delays, and >=1 cycle')
    if os.name != 'nt' and os.environ.get('GITHUB_ACTIONS') != 'true':
        parser.error('Unix desktop launches are CI only')
    if subprocess.run(['git', 'diff', '--quiet', 'HEAD', '--', 'perf', 'rust', 'src', 'electron',
                       'shared', 'package.json', 'bun.lock', 'vite.config.ts']).returncode != 0:
        parser.error('commit measured sources before qualification')
    desktop = HiddenDesktop() if os.name == 'nt' else None
    runs = []
    args.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        for index in range(args.runs):
            pair = {}
            for kind in (('native', 'electron') if index % 2 == 0 else ('electron', 'native')):
                print(f'pair {index + 1}: {kind}', flush=True)
                try:
                    pair[kind] = run_one(kind, getattr(args, kind).resolve(), desktop, args)
                except Exception as error:
                    args.output.write_text(json.dumps({'schema': 1, 'status': 'error', 'runtime': kind,
                        'error': str(error), 'completedPairs': runs, 'partialPair': pair}, indent=2), encoding='utf-8')
                    raise
                print(json.dumps({k: v for k, v in pair[kind].items() if k != 'memorySamples'}), flush=True)
            runs.append(pair)
    finally:
        if desktop:
            desktop.close()
    summary = summarize(runs)
    # Same-runner Electron control is the regression baseline. Broad allowances
    # absorb hosted GPU/scheduling jitter. Preserve every raw run for diagnosis.
    failures = evaluate(summary)
    result = {'schema': 1, 'status': 'fail' if failures else 'pass', 'platform': platform.platform(), 'architecture': platform.machine(),
              'capturedAt': datetime.now().astimezone().isoformat(),
              'appVersion': json.loads((ROOT / 'package.json').read_text(encoding='utf-8'))['version'],
              'psutilVersion': psutil.__version__, 'websocketVersion': websocket.__version__,
              'runnerImage': os.environ.get('ImageOS'), 'runnerImageVersion': os.environ.get('ImageVersion'),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'executables': {kind: {'sha256': hashlib.file_digest(getattr(args, kind).open('rb'), 'sha256').hexdigest()}
                              for kind in ('native', 'electron')},
              'fixtureSha256': hashlib.sha256(json.dumps(CONTRACT, sort_keys=True).encode()).hexdigest(),
              'options': vars(args) | {'native': str(args.native), 'electron': str(args.electron), 'output': str(args.output)},
              'runs': runs, 'medians': summary, 'failures': failures}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    write_report(args.output, summary)
    print(json.dumps(summary, indent=2))
    if failures:
        raise SystemExit(f'Performance budgets exceeded: {failures}')


if __name__ == '__main__':
    main()
