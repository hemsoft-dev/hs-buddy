"""Actual installed Copilot turns and native UI on disposable CI hosts only."""
import argparse
import base64
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from urllib.parse import parse_qs, urlparse
import uuid

from desktop import fixture
import importlib.util
HERE = Path(__file__).resolve().parent

def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), HERE / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

terminal = load('terminal-desktop')
github, settings = terminal.github, terminal.settings

class CopilotGitHubBackend(github.GitHubBackend):
    def __init__(self):
        super().__init__(accounts=('synthetic-account',))

    def respond(self, account, method, path, body):
        route, query = urlparse(path).path, parse_qs(urlparse(path).query)
        if account == 'synthetic-account' and method == 'GET':
            if route == '/copilot-fixture/codex-usage':
                now = int(time.time())
                return 200, dict(plan_type='synthetic', rate_limit=dict(primary_window=dict(used_percent=25, reset_at=now + 9000, limit_window_seconds=18000), secondary_window=dict(used_percent=14, reset_at=now + 302400, limit_window_seconds=604800))), {}
            if route.endswith('/copilot/billing'):
                return 200, {'seat_breakdown': {'total': 1}}, {}
            if route.endswith('/settings/billing/usage'):
                year, month = int(query['year'][0]), int(query['month'][0])
                return 200, {'usageItems': [dict(date=f'{year}-{month:02}-02', product='copilot', sku='Copilot AI Credits', quantity=1200, grossAmount=12, discountAmount=3, netAmount=9)]}, {}
            if route.startswith('/organizations/') and route.endswith('/budgets'):
                return 200, {'budgets': [], 'has_next_page': False}, {}
            if route.startswith('/enterprises/') and route.endswith('/budgets'):
                page = int(query.get('page', ['1'])[0])
                return 200, {'budgets': [] if page == 1 else [dict(budget_entity_name='fixture-org', budget_product_sku='premium_requests', budget_amount=200, prevent_further_usage=True)], 'has_next_page': page == 1}, {}
        status, value, headers = super().respond(account, method, path, body)
        if 'NativePull(' in (body or {}).get('query', '') and status == 200:
            value['data']['repository']['pullRequest']['body'] += ' BUDDYSTREAM61527'
        return status, value, headers

@contextmanager
def environment(changes):
    previous = {key: os.environ.get(key) for key in changes}
    try:
        os.environ.update({key: str(value) for key, value in changes.items()})
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

@contextmanager
def streaming_capture(output):
    capture = output / 'chat-dark' / 'streaming-dark.png'
    if capture.exists() or capture.is_symlink():
        raise RuntimeError('Preserve the existing streaming capture; qualification needs a fresh output')
    with environment(dict(BUDDY_COPILOT_STREAM_CAPTURE=capture)):
        yield capture

def write_section(config, section, appearance=None):
    value = json.loads(config.read_text(encoding='utf-8-sig'))
    value['native']['activeSection'] = section
    if appearance:
        value['ui'].update(appearance)
    config.write_text(json.dumps(value), encoding='utf-8')

def session_fixture(output):
    user = output / 'editor User directory 界'
    identifier = str(uuid.uuid4())
    workspace = user / 'workspaceStorage' / 'synthetic-workspace'
    sessions = workspace / 'chatSessions'
    sessions.mkdir(parents=True)
    (workspace / 'workspace.json').write_text(json.dumps({'folder': 'file:///synthetic/project'}))
    prompts = ['Review a synthetic change'] + [f'Inspect synthetic request {i}' for i in range(2, 21)] + ['Final synthetic request twenty one']
    lines = [dict(kind=0, v=dict(sessionId=identifier, creationDate=1792152000123,
        customTitle='Synthetic session', requests=[dict(message=dict(text=prompt)) for prompt in prompts],
        inputState=dict(selectedModel=dict(metadata=dict(id='gpt-4.1', name='Synthetic Model', multiplierNumeric=1)))))]
    for index in range(len(prompts)):
        lines.append(dict(kind=1, k=['requests', index, 'result'], v=dict(
            metadata=dict(promptTokens=100 if index == 0 else 0, outputTokens=25 if index == 0 else 0),
            timings=dict(firstProgress=10 if index == 0 else 0, totalElapsed=800 if index == 0 else 0))))
    path = sessions / (identifier + '.jsonl')
    path.write_text('\n'.join(json.dumps(line) for line in lines), encoding='utf-8')
    expected = hashlib.sha256(path.read_bytes()).hexdigest()
    (output / 'session-source-fixture.json').write_text(json.dumps(dict(
        fileName=path.name, expectedSha256=expected), indent=2), encoding='utf-8')
    return user, path, expected


@contextmanager
def session_source_guard(source, expected, output):
    """Check original bytes even when UI navigation fails; never restore them."""
    primary = None
    try:
        yield
    except BaseException as failure:
        primary = failure
        raise
    finally:
        try:
            actual = hashlib.sha256(source.read_bytes()).hexdigest()
            (output / 'session-source-integrity.json').write_text(json.dumps(dict(
                result='passed' if actual == expected else 'failed',
                expectedSha256=expected, actualSha256=actual), indent=2), encoding='utf-8')
            if actual != expected:
                raise RuntimeError('Read-only session exploration changed its source file')
        except BaseException as verification_failure:
            if primary is None:
                raise
            primary.add_note(f'Session source verification failed: {type(verification_failure).__name__}')

def provider_receipts(output):
    rows = [json.loads(path.read_text()) for path in output.glob('copilot-provider-*.json')]
    if not rows or any(row['offline'] is not True or row['upstreamForwarder'] is not False or row['rejected'] != 0 for row in rows):
        raise RuntimeError('The SDK provider receipt is missing or contains an unexpected upstream request')
    return rows

def assert_no_owned_children(output):
    if os.environ.get('GITHUB_ACTIONS') != 'true':
        raise RuntimeError('Process inspection requires the disposable CI host')
    deadline = time.monotonic() + 10
    while True:
        if os.name == 'nt':
            encoded = base64.b64encode(str(output).encode()).decode()
            script = "$root=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('" + encoded + "')); @(Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('buddy.exe','buddy-native.exe','copilot.exe','copilot-runtime.exe','node.exe') -and $_.CommandLine -and $_.CommandLine.IndexOf($root,[StringComparison]::OrdinalIgnoreCase) -ge 0 } | Select-Object -ExpandProperty ProcessId) | ConvertTo-Json -Compress"
            raw = subprocess.check_output(['pwsh', '-NoProfile', '-Command', script], text=True).strip()
            pids = json.loads(raw) if raw else []
            if isinstance(pids, int):
                pids = [pids]
        else:
            rows = subprocess.check_output(['ps', '-axo', 'pid=,comm=,args='], text=True).splitlines()
            pids = []
            for row in rows:
                fields = row.strip().split(None, 2)
                if len(fields) == 3 and str(output) in fields[2] and any(name in Path(fields[1]).name for name in ('buddy', 'copilot', 'node')):
                    pids.append(int(fields[0]))
        if not pids:
            return
        if time.monotonic() >= deadline:
            raise RuntimeError('An owned Copilot SDK/helper process survived app shutdown')
        time.sleep(.1)

def assert_private_content_not_logged(output):
    paths = list(output.glob('*/app.log'))
    if not paths:
        raise RuntimeError('Actual app diagnostics were not captured for privacy validation')
    for path in paths:
        text = path.read_text(encoding='utf-8', errors='replace')
        if any(marker in text for marker in ('BUDDYSTREAM61527', 'BUDDYCANCEL73429', 'Partial synthetic answer', 'fixture-member', 'Review a synthetic change')):
            raise RuntimeError('Private synthetic assistant/session data appeared in app diagnostics')


STARTUP_SCENES = ('chat-dark', 'review-dark', 'usage-light', 'codex-auth-error-light', 'sessions-light')


def assert_startup_logs_clean(output):
    """No baseline allowance: either GPUI diagnostic blocks installed qualification."""
    counts = {}
    for scene in STARTUP_SCENES:
        path = output / scene / 'app.log'
        text = path.read_text(encoding='utf-8', errors='replace')
        counts[scene] = {marker: text.count(marker) for marker in (
            'RefCell already borrowed', 'unable to get cursor position')}
    receipt = dict(result='failed' if any(sum(row.values()) for row in counts.values()) else 'passed', scenes=counts)
    (output / 'startup-diagnostics.json').write_text(json.dumps(receipt, indent=2), encoding='utf-8')
    if receipt['result'] != 'passed':
        raise RuntimeError('Native startup diagnostics failed; inspect startup-diagnostics.json and original app.log files')
    return receipt

def headless(executable, output, config, backend):
    env = dict(os.environ, BUDDY_CONFIG_PATH=str(config), BUDDY_CONVEX_URL=backend.url,
        GH_CONFIG_DIR=str(output / 'empty-gh'))
    for key in ('GH_TOKEN', 'GITHUB_TOKEN', 'NODE_OPTIONS', 'ELECTRON_RUN_AS_NODE'):
        env.pop(key, None)
    result = subprocess.run([str(executable), '--qualify-copilot', str(output / 'native-assistant')],
        cwd=output, env=env, capture_output=True, text=True, timeout=300,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
    (output / 'headless.stdout').write_text(result.stdout, encoding='utf-8')
    (output / 'headless.stderr').write_text(result.stderr, encoding='utf-8')
    if result.returncode:
        raise RuntimeError('The installed headless SDK runner failed; inspect its owned output')
    receipt = json.loads(result.stdout)
    for key in ('actualSdkStream', 'unicodeDeltas', 'unsavedNoRecord', 'cancelPartialRetained', 'savedOutputReadBack', 'savedCancellationReadBack', 'activeHandleDropStopped', 'unauthenticatedSessionRejected', 'retainedSessionTransportStopped'):
        if receipt.get(key) is not True:
            raise RuntimeError('The installed SDK runner did not prove ' + key)
    # Two saved turns, with completed output and cancelled partial output.
    results = backend.query('copilotResults:listRecent')
    if len(results) != 2 or sorted(row['status'] for row in results) != ['completed', 'failed']:
        raise RuntimeError('The unsaved turns created records, or a saved turn was lost')
    rows = provider_receipts(output)
    if sum(row['requests'] for row in rows) != 5 or sum(row['cancelled'] for row in rows) < 2:
        raise RuntimeError('Actual SDK request/cancellation counts differ from the five explicit turns')
    return receipt

def chat_turns(driver):
    dark = driver.drive('chat-dark', [
        ('input', 'GitHub account', 'synthetic-account'), ('click', 'Load available models'),
        ('wait_models', 'Synthetic offline model'), ('click', 'Synthetic offline model'), ('input', 'Message', 'BUDDYSTREAM61527'),
        ('click', 'Run and save'), ('wait_text', 'Assistant is responding'), ('capture', 'streaming-dark'),
        ('wait_scroll_text', 'Saved to results'), ('reveal_text', 'Copy answer', 'Saved to results'),
        ('click', 'Copy answer'), ('capture', 'completed-dark'),
        ('reveal_text', 'Inspect saved result', 'Saved to results'), ('click', 'Inspect saved result'),
        ('wait_text', 'Synthetic assistant'), ('capture', 'saved-result-dark'),
        ('click', 'Chat'), ('click', 'New conversation'), ('input', 'Message', 'BUDDYCANCEL73429'),
        ('click', 'Run and save'), ('wait_text', 'Partial synthetic answer'), ('click', 'Cancel response'),
        ('wait_scroll_text', 'Cancelled'), ('wait_scroll_text', 'Saved to results'), ('resize', 900, 650),
        ('reveal_text', 'Cancelled', 'Assistant'), ('wait_text', 'Partial synthetic answer'), ('capture', 'cancelled-compact-dark'),
    ], record=True)
    github.check(dark / 'streaming-dark.png', 'Assistant is responding')
    github.check(dark / 'completed-dark.png', 'Answer copied', 'Saved to results')
    github.check(dark / 'saved-result-dark.png', 'Completed', 'Synthetic assistant')
    github.check(dark / 'cancelled-compact-dark.png', 'Cancelled', 'Partial synthetic answer')


def review_pull_request(driver):
    review = driver.drive('review-dark', [
        ('click', '#7 Second private navigation review'), ('click', 'Review with Buddy assistant'),
        ('wait_text', 'Reviewing fixture-org/native-demo'),
        ('click', 'Load available models'), ('wait_models', 'Synthetic offline model'), ('click', 'Synthetic offline model'),
        ('capture', 'review-context-dark'),
        ('click', 'Run and save'), ('wait_scroll_text', 'Saved to results'),
        ('reveal_text', 'Inspect saved result', 'Saved to results'), ('click', 'Inspect saved result'),
        ('wait_text', 'Captured review'), ('capture', 'review-result-dark'),
    ])
    github.check(review / 'review-context-dark.png', 'synthetic-account', 'Run and save')
    github.check_column(review / 'review-result-dark.png', 'Captured review',
        'Captured review', 'Freshness has not been checked')


def usage_pages(driver):
    usage = driver.drive('usage-light', [
        ('click', 'Usage'), ('click', 'Refresh usage'), ('wait_text', 'Billing snapshot saved'),
        ('click', 'Load period'), ('wait_text', 'Showing'), ('capture', 'usage-history-light'),
        ('wheel', -600), ('click', 'Load metrics file'), ('wait_scroll_text', 'fixture-member'), ('capture', 'enterprise-users-light'),
        ('wheel', 1200), ('click', 'codex-fixture'), ('click', 'Refresh usage'),
        ('wait_text', 'Weekly allowance'), ('capture', 'codex-allowances-light'),
    ])
    # Both OCR engines read this font's "AI" prefix as "Al" on the actual
    # Mac capture. Require the complete fixture allowance and used count.
    github.check(usage / 'usage-history-light.png', 'Credits: 1200 used / 3900 included', 'Projected period spend', 'Copilot billing history')
    github.check(usage / 'enterprise-users-light.png', 'Enterprise users', 'fixture-member')
    github.check(usage / 'codex-allowances-light.png', 'Weekly allowance', '5-hour allowance', 'remaining')


def qualify(executable, output, session, version, backend, api):
    binary = github.installed_binary(executable, version)
    config = fixture(output, 'dark')
    value = json.loads(config.read_text())
    value['github']['accounts'] = [dict(username='synthetic-account', org='fixture-org', usageProvider='copilot'), dict(username='codex-fixture', org='fixture-org', usageProvider='codex')]
    value['copilot'] = {'model': 'gpt-4.1'}
    value['ui']['enterpriseSlug'] = 'fixture-enterprise'
    value['native']['activeSection'] = 'copilot'
    config.write_text(json.dumps(value))
    (output / 'copilot-fixture.allow').write_text('hs-buddy native Copilot qualification v1\n' + str(uuid.uuid4()))
    (output / 'github-fixture.allow').write_text('hs-buddy native GitHub qualification v1')
    metrics = output / 'enterprise-metrics.json'
    metrics.write_text(json.dumps({'Enterprise': 'Synthetic enterprise', 'Organization': 'fixture-org', 'Users': [dict(User='fixture-member', grossQuantity=19, grossAmount=.19, netAmount=.1, topModel='Synthetic Model')]}))
    user_directory, transcript, transcript_sha = session_fixture(output)
    github.mutate(backend, 'githubAccounts:create', dict(username='synthetic-account', org='fixture-org', usageProvider='copilot'))
    github.mutate(backend, 'githubAccounts:create', dict(username='codex-fixture', org='fixture-org', usageProvider='codex'))
    driver = settings.Driver(executable, output, session, backend, HERE.parent.parent)
    # GetCursorPos requires the input desktop. The driver independently rejects
    # this mode outside disposable CI; laptop probes must retain hidden desktops.
    driver.input_desktop = session == 'windows'
    sentinel = terminal.start_sentinel()
    primary = None
    try:
        with environment(dict(BUDDY_COPILOT_FIXTURE='1', BUDDY_CONFIG_PATH=config, BUDDY_CONVEX_URL=backend.url,
                CODEX_HOME=output / 'empty-codex', BUDDY_CODEX_FIXTURE_SCENARIO='normal', COPILOT_METRICS_FILE=metrics,
                RUST_LOG='warn,github_copilot_sdk=trace,convex=trace,tungstenite=trace,tokio_tungstenite=trace')), github.fixture_environment(api.url), \
                session_source_guard(transcript, transcript_sha, output):
            sdk = headless(executable, output, config, backend)
            assert_no_owned_children(output)
            terminal.verify_sentinel(sentinel, 'headless SDK cleanup')
            with streaming_capture(output):
                chat_turns(driver)
            terminal.verify_sentinel(sentinel, 'chat app shutdown')
            assert_no_owned_children(output)
            write_section(config, 'github')
            review_pull_request(driver)
            light = json.loads(fixture(output / 'light-template', 'light').read_text())['ui']
            write_section(config, 'copilot', light)
            usage_pages(driver)
            with environment(dict(BUDDY_CODEX_FIXTURE_SCENARIO='missing-auth')):
                missing = driver.drive('codex-auth-error-light', [('click', 'Usage'), ('click', 'codex-fixture'), ('click', 'Refresh usage'), ('wait_text', 'No Codex ChatGPT login found'), ('capture', 'codex-auth-error-light')])
                github.check(missing / 'codex-auth-error-light.png', 'No Codex ChatGPT login found')
            sessions = driver.drive('sessions-light', [
                ('click', 'Sessions'), ('input', 'VS Code User directory', str(user_directory), 'Scan folder'), ('click', 'Scan folder'),
                ('wait_text', 'Synthetic session'), ('click', 'Synthetic session'), ('wait_text', '100 prompt tokens'),
                ('capture', 'session-detail-light'), ('resize', 900, 650), ('capture', 'session-compact-light'),
                ('wait_scroll_above', 'Inspect synthetic request 20', 'Next requests'),
                ('input', 'VS Code User directory', str(output / 'missing-directory'), 'Scan folder'), ('click', 'Scan folder'),
                ('wait_text', 'unavailable'), ('click', 'Next requests'),
                ('wait_scroll_above', 'Final synthetic request twenty one', 'Next requests'), ('capture', 'session-error-light'),
                ('click', 'Previous requests'), ('wait_scroll_above', 'Inspect synthetic request 20', 'Next requests'),
                ('wait_text', 'unavailable'),
            ])
            github.check(sessions / 'session-detail-light.png', '100 prompt tokens', '25 output tokens', '0.25')
            github.check(sessions / 'session-error-light.png', 'unavailable', 'Loaded source')
            terminal.verify_sentinel(sentinel, 'all native Copilot app shutdowns')
            assert_no_owned_children(output)
            (output / 'requests.json').write_text(json.dumps(api.requests, indent=2))
            assert_private_content_not_logged(output)
            startup = assert_startup_logs_clean(output)
            qualification = dict(result='passed', version=version, session=session,
                startupDiagnostics=startup,
                **binary, largeWindow=driver.large_size, headless=sdk, privateTransportTraceContentAbsent=True, ownedSdkChildrenAbsent=True, unrelatedSentinelAlive=True, readOnlySessionSha256=transcript_sha,
                sessionRequestPaging=dict(requests=21, compactWindow=[900, 650], nextAndPreviousActivated=True, scrollContentReached=True, sourceErrorRetained=True),
                dataSource='Synthetic offline SDK provider, scoped loopback GitHub, and isolated anonymous Convex')
    except BaseException as failure:
        primary = failure
        github.preserve_failed_requests(output, api.requests, failure)
        raise
    finally:
        terminal.stop_sentinel(sentinel, primary)
    # A pass receipt is only valid after source verification and cleanup finish.
    (output / 'qualification.json').write_text(json.dumps(qualification, indent=2))

def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('BUDDY_SETTINGS_FIXTURE') != '1':
        raise RuntimeError('Copilot qualification requires a disposable GitHub Actions runner')
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--session', choices=('windows', 'macos', 'x11'), required=True)
    parser.add_argument('--version', required=True)
    args = parser.parse_args()
    output = terminal.owned_output(args.output)
    with settings.SettingsBackend(HERE.parent.parent) as backend, CopilotGitHubBackend() as api:
        qualify(args.executable.resolve(strict=True), output, args.session, args.version, backend, api)

if __name__ == '__main__':
    main()
