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
SESSION_SOURCE_ERROR = 'The selected VS Code session directory is unavailable.'

def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), HERE / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

terminal = load('terminal-desktop')
session_workspaces = load('session-workspaces')
github, settings = terminal.github, terminal.settings

class CopilotGitHubBackend(github.GitHubBackend):
    def __init__(self):
        super().__init__(accounts=('synthetic-account',))
        self.publication_ack = True

    def respond(self, account, method, path, body):
        route, query = urlparse(path).path, parse_qs(urlparse(path).query)
        if account == 'synthetic-account' and method == 'GET':
            if route == '/copilot-fixture/codex-usage':
                now = int(time.time())
                return 200, dict(plan_type='synthetic', rate_limit=dict(primary_window=dict(used_percent=25, reset_at=now + 9000, limit_window_seconds=18000), secondary_window=dict(used_percent=14, reset_at=now + 302400, limit_window_seconds=604800))), {}
            if route == '/copilot_internal/user':
                # The premium usage badge's quota: 126 of 300 used.
                return 200, {'quota_reset_date_utc': '2099-01-01T00:00:00Z', 'quota_snapshots': {'premium_interactions': dict(
                    entitlement=USAGE_ENTITLEMENT, remaining=USAGE_REMAINING, overage_count=0, percent_remaining=58)}}, {}
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
        if method == 'POST' and route == '/repos/fixture-org/native-demo/issues/7/comments' and (body or {}).get('body', '').startswith('## 🤖 AI Review\n') and not self.publication_ack:
            # The fixture has already stored the comment. Lose its acknowledgement.
            return 201, {}, {}
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
        if any(marker in text for marker in ('BUDDYSTREAM61527', 'BUDDYCANCEL73429', CODE_MARKER, 'Partial synthetic answer', 'fixture-member', 'Review a synthetic change')):
            raise RuntimeError('Private synthetic assistant/session data appeared in app diagnostics')


USAGE_ENTITLEMENT, USAGE_REMAINING = 300, 174
# The badge reads "42% · 174/300"; only the figures are matched.
USAGE_BADGE = f'{USAGE_REMAINING}/{USAGE_ENTITLEMENT}'
# The sidebar row of the review-dark result, "PR Review: <title>". The sidebar
# cuts the title to its width, as Electron's does: the 1044 px Windows window
# shows "PR Review: Se..." (run 46007700214), so only the prefix is matched.
# Chat's "PR Review" category button is a complete label and would win over
# this row, so the scene opens the section from a pane without it.
RECENT_REVIEW_LABEL = 'PR Review'
# The colon tells the row from Saved results' "PR reviews" filter. Windows OCR
# reads it as a full stop. The title's first letters, "Se", are on the row at
# every width the scenes use, so a row that lost its title ("PR Review:
# Untitled") fails the check.
RECENT_REVIEW_MARKER = 'PR Review: Se|PR Review. Se'
RECENT_REVIEW_ABSENT = ('Untitled',)
# The opened result's first control, above everything it scrolls.
RESULT_TOP = 'Refresh this result'
# Any of these sits inside the opened result pane, whichever is in view after
# it scrolls: wheel there, never at the driver's default point. Beside the
# Copilot sidebar that point (600, 400) lies on the short results list, which
# does not scroll (Ubuntu: rerun-dark in run 46007700214, review-dark's
# 'Confirm publication' in run 46084940944).
RESULT_PANE = 'Publish to PR|Copy answer|Refresh this result'
CODE_MARKER = 'BUDDYCODE48213'
CODE_LINE = 'copy only this'
UNCONFIRMED_ENDING = 'will not resend'
UNCONFIRMED_LAST_LINE_WORD = 'resend'
STARTUP_SCENES = ('chat-dark', 'rerun-dark', 'code-copy-dark', 'review-dark', 'recent-results-dark', 'session-workspaces-dark', 'review-unconfirmed-light', 'usage-light', 'code-copy-light', 'codex-auth-error-light', 'sessions-light')


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
        ('input', 'GitHub account', 'synthetic-account'), ('content', 'Load available models'),
        ('wait_models', 'Synthetic offline model'), ('content', 'Synthetic offline model'), ('input', 'Message', 'BUDDYSTREAM61527'),
        # Electron's category is chosen before the run and stored on the result.
        ('content', 'Documentation'),
        ('content', 'Run and save'), ('wait_text', 'Assistant is responding'), ('wait_text', '1 active'), ('capture', 'streaming-dark'),
        ('wait_scroll_text', 'Saved to results'), ('reveal_text', 'Copy answer', 'Saved to results'),
        ('content', 'Copy answer'), ('capture', 'completed-dark'),
        ('reveal_text', 'Inspect saved result', 'Saved to results'), ('content', 'Inspect saved result'),
        ('wait_text', 'Synthetic assistant'), ('capture', 'saved-result-dark'),
        # A new conversation starts from the saved account or the active gh
        # login; the runner has neither, so name the synthetic account again.
        ('content', 'Chat'), ('content', 'New conversation'), ('input', 'GitHub account', 'synthetic-account'),
        # RECENTROW tells this saved prompt apart from the headless runner's.
        ('input', 'Message', 'BUDDYCANCEL73429 RECENTROW'),
        ('content', 'Run and save'), ('wait_text', 'Partial synthetic answer'), ('content', 'Cancel response'),
        ('wait_scroll_text', 'Cancelled'), ('wait_scroll_text', 'Saved to results'), ('resize', 900, 650),
        # At 900x650 the conversation keeps its 160 px minimum and scrolls to
        # the end, so the "Assistant" label sits above it; anchor on the answer.
        ('reveal_text', 'Cancelled', 'Partial synthetic answer'), ('wait_text', 'Partial synthetic answer'), ('capture', 'cancelled-compact-dark'),
    ], record=True)
    github.check(dark / 'streaming-dark.png', 'Assistant is responding')
    github.check(dark / 'streaming-dark.png', '1 active')
    github.check(dark / 'completed-dark.png', 'Answer copied', 'Saved to results', 'Documentation')
    github.check(dark / 'saved-result-dark.png', 'Completed', 'Synthetic assistant', 'Re-run this prompt')
    github.check(dark / 'cancelled-compact-dark.png', 'Cancelled', 'Partial synthetic answer')


def rerun_turns(driver):
    """Open the cancelled prompt from Recent, re-run it and cancel the re-run.

    A fresh app session after the streaming capture: the provider permits only
    one streaming turn while that capture is armed, and the cancel marker never
    streams to completion, so the re-run ends by its own Cancel."""
    rerun = driver.drive('rerun-dark', [
        ('wait_scroll_text', 'RECENTROW'), ('content', 'RECENTROW'),
        ('wait_text', 'Re-run this prompt'), ('capture', 'recent-opened-dark'),
        ('content', 'Re-run this prompt'), ('wait_text', 'Re-running this prompt'), ('sleep', 3),
        ('content', 'Cancel re-run'), ('wait_scroll_text', 'Re-run saved as a new result'), ('capture', 'rerun-saved-dark'),
        # The saved answer sits below the failure notice and its actions.
        # Wheel over the opened result, measured by its top button: beside
        # the Copilot sidebar the default point (600, 400) of a 1200 px
        # window is on the short results list, which does not scroll
        # (Ubuntu, run 46007700214).
        ('content', 'Open re-run result'), ('wait_scroll_text', 'Partial synthetic answer', RESULT_TOP),
        ('capture', 'rerun-opened-dark'),
    ], record=True)
    github.check(rerun / 'recent-opened-dark.png', 'RECENTROW', 'Re-run this prompt')
    github.check(rerun / 'rerun-saved-dark.png', 'Re-run cancelled', 'Re-run saved as a new result', 'Open re-run result')
    # Scrolled to the answer, the title may be above the view; the backend
    # check ties the record to its prompt.
    github.check(rerun / 'rerun-opened-dark.png', 'Partial synthetic answer')


def require_prompt_records(backend, started_ms):
    """The chosen category reached the desktop run, and the re-run copied its source's.

    The headless runner saved the same marker prompts earlier, so only rows
    created by the desktop scenes count."""
    rows = [row for row in backend.query('copilotResults:listRecent') if row.get('createdAt', 0) >= started_ms]
    # Native Chat saves the conversation: a preamble, then the user's turn.
    streamed = [row for row in rows if row.get('prompt', '').endswith('User: BUDDYSTREAM61527')]
    if len(streamed) != 1 or streamed[0].get('category') != 'documentation' or streamed[0]['status'] != 'completed':
        raise RuntimeError('The chosen Documentation category did not reach the saved result')
    recent = [row for row in rows if row.get('prompt', '').endswith('User: BUDDYCANCEL73429 RECENTROW')]
    if len(recent) != 2 or len({row.get('category') for row in recent}) != 1:
        raise RuntimeError('The re-run did not save one new result with its source category')


def code_copy(driver, appearance):
    # The marker answers with a heading and one fenced block. Capture its Copy
    # code control beside the first code line, then activate it on the
    # installed app. Headless GPUI tests prove the copied text.
    name = 'code-copy-' + appearance
    captures = [name]
    # Electron's premium usage badge beside the account field.
    badge = 'copilot-usage-badge-' + appearance
    if appearance == 'light':
        # At 900x650 the chat pane is shorter than one wheel step, so use the
        # saved result's taller pane. Revealing the code line keeps the control
        # just above it in view.
        captures.append('code-copy-compact-light')
        reveal = [('reveal_text', 'Inspect saved result', 'Saved to results'), ('content', 'Inspect saved result'),
            ('wait_text', 'Synthetic assistant'), ('reveal_text', CODE_LINE, 'Copy prompt'), ('capture', name),
            ('resize', 900, 650), ('reveal_text', CODE_LINE, 'Copy prompt'), ('capture', captures[1])]
    else:
        reveal = [('wait_scroll_up', 'Copy code', 'Saved to results'), ('capture', name)]
    scene = driver.drive(name, [
        ('input', 'GitHub account', 'synthetic-account'),
        *([('wait_text', USAGE_BADGE), ('capture', badge)] if appearance == 'light' else []),
        ('content', 'Load available models'),
        ('wait_models', 'Synthetic offline model'), ('content', 'Synthetic offline model'),
        ('input', 'Message', 'BUDDYSTREAM61527 ' + CODE_MARKER), ('content', 'Run and save'),
        ('wait_scroll_text', 'Saved to results'), *reveal, ('content', 'Copy code'),
    ], record=appearance == 'dark')
    if appearance == 'light':
        github.check(scene / (badge + '.png'), 'GitHub account', USAGE_BADGE)
    for capture in captures:
        github.check(scene / (capture + '.png'), 'Copy code', CODE_LINE)


def recent_results(driver):
    """The premium usage badge beside the account, then the sidebar's Recent
    Results section: open it, find the review-dark result and open it."""
    scene = driver.drive('recent-results-dark', [
        ('input', 'GitHub account', 'synthetic-account'), ('wait_text', USAGE_BADGE),
        ('capture', 'copilot-usage-badge-dark'),
        # Sessions has no "PR Review" label, so the sidebar row is the only
        # one. A row still opens its result in Saved results from any pane.
        ('content', 'Sessions'),
        # Electron's section starts collapsed. Wait for the row before
        # clicking it: the list is read again when the section opens.
        ('content', 'Recent Results'), ('wait_text', RECENT_REVIEW_LABEL), ('sleep', 1),
        ('capture', 'copilot-recent-results-dark'),
        ('content', RECENT_REVIEW_LABEL), ('wait_text', 'Captured review'),
        ('capture', 'copilot-recent-result-opened-dark'),
    ], record=True)
    github.check(scene / 'copilot-usage-badge-dark.png', 'GitHub account', USAGE_BADGE)
    github.check(scene / 'copilot-recent-results-dark.png', 'Recent Results', RECENT_REVIEW_MARKER,
        absent=RECENT_REVIEW_ABSENT)
    github.check(scene / 'copilot-recent-result-opened-dark.png', RECENT_REVIEW_MARKER, 'Captured review',
        absent=RECENT_REVIEW_ABSENT)


def review_pull_request(driver):
    review = driver.drive('review-dark', [
        ('content', '#7 Second private navigation review'), ('content', 'Review with Buddy assistant'),
        ('wait_text', 'Reviewing fixture-org/native-demo'),
        ('content', 'Load available models'), ('wait_models', 'Synthetic offline model'), ('content', 'Synthetic offline model'),
        ('capture', 'review-context-dark'),
        ('content', 'Run and save'), ('wait_scroll_text', 'Saved to results'),
        ('reveal_text', 'Inspect saved result', 'Saved to results'), ('content', 'Inspect saved result'),
        ('wait_text', 'Captured review'), ('wait_text', 'Open PR on GitHub'), ('capture', 'review-result-dark'),
        ('reveal_text', 'Publish to PR', 'Captured review'), ('content', 'Publish to PR'),
        ('wait_scroll_text', 'Confirm publication', RESULT_PANE), ('capture', 'publish-confirmation-dark'),
        ('content', 'Cancel publication'), ('wait_scroll_up', 'Publish to PR'), ('content', 'Publish to PR'),
        ('content', 'Confirm publication'), ('wait_scroll_text', 'Review comment published', RESULT_PANE),
        ('capture', 'publish-acknowledged-dark'), ('wait_scroll_up', 'Refresh this result', 'Review comment published'), ('content', 'Refresh this result'),
        ('wait_scroll_text', 'Review comment published', RESULT_PANE), ('capture', 'publish-retained-dark'),
    ], record=True)
    github.check(review / 'review-context-dark.png', 'synthetic-account', 'Run and save')
    github.check_column(review / 'review-result-dark.png', 'Captured review',
        'Captured review', 'Freshness has not been checked')
    # Electron's result header action for a pr-review result. The scene does not
    # click it: opening a browser on the CI host is outside the owned output.
    github.check(review / 'review-result-dark.png', 'Open PR on GitHub')
    github.check(review / 'publish-confirmation-dark.png', 'synthetic-account', 'Confirm publication')
    github.check(review / 'publish-retained-dark.png', 'Review comment published')


def review_publication_unconfirmed(driver):
    review = driver.drive('review-unconfirmed-light', [
        ('content', '#7 Second private navigation review'), ('content', 'Review with Buddy assistant'),
        ('wait_text', 'Reviewing fixture-org/native-demo'), ('content', 'Load available models'),
        ('wait_models', 'Synthetic offline model'), ('content', 'Synthetic offline model'),
        ('content', 'Run and save'), ('wait_scroll_text', 'Saved to results'),
        ('reveal_text', 'Inspect saved result', 'Saved to results'), ('content', 'Inspect saved result'),
        ('wait_text', 'Captured review'), ('resize', 900, 650),
        ('reveal_text', 'Publish to PR', 'Captured review'), ('content', 'Publish to PR'),
        ('wait_scroll_text', 'Confirm publication', RESULT_PANE), ('capture', 'publish-confirmation-compact-light'),
        ('content', 'Confirm publication'), ('wait_scroll_text', 'Publication unconfirmed', RESULT_PANE),
        ('capture', 'publish-unconfirmed-compact-light'), ('wait_scroll_up', 'Refresh this result', 'Publication unconfirmed'), ('content', 'Refresh this result'),
        # The two-line status ends below a 650px window on Ubuntu; wait for its
        # last line so the capture holds the whole status, not its first line.
        # A wait matches within one OCR line, and the Mac font breaks the
        # status between "will" and "not resend it" (run 44786129019), so
        # wait for one word of that line. The check below joins lines.
        ('wait_scroll_text', UNCONFIRMED_LAST_LINE_WORD, RESULT_PANE), ('capture', 'publish-unconfirmed-retained-light'),
    ], record=True)
    github.check(review / 'publish-unconfirmed-retained-light.png', 'Publication unconfirmed', UNCONFIRMED_ENDING)


def require_review_publications(requests, results):
    writes = [request for request in requests if request['method'] == 'POST' and request['path'] != '/graphql']
    if len(writes) != 2:
        raise RuntimeError('Expected exactly two explicit saved-review publications, with no refresh replay')
    expected = sorted('## 🤖 AI Review\n\n' + result['result'] + '\n\n---\n*Published from HS Buddy — ' + (result.get('model') or 'AI') + ' review*'
        for result in results if result.get('category') == 'pr-review' and result['status'] == 'completed')
    if len(expected) != 2 or sorted((request.get('body') or {}).get('body', '') for request in writes) != expected:
        raise RuntimeError('Saved-review publication did not send the exact stored answers and model attribution')
    for request in writes:
        body = (request.get('body') or {}).get('body', '')
        if request['account'] != 'synthetic-account' or request['path'] != '/repos/fixture-org/native-demo/issues/7/comments' or not body.startswith('## 🤖 AI Review\n\n') or 'Synthetic assistant' not in body or '*Published from HS Buddy' not in body:
            raise RuntimeError('Saved-review publication has the wrong account, destination or body')
    return dict(attempts=2, acknowledged=1, unconfirmed=1, refreshReplay=0)


def usage_pages(driver):
    usage = driver.drive('usage-light', [
        ('content', 'Usage'), ('content', 'Refresh usage'), ('wait_text', 'Billing snapshot saved'),
        ('content', 'Load period'), ('wait_text', 'Showing'), ('wait_text', 'Open Copilot settings on GitHub'),
        ('capture', 'usage-history-light'),
        ('wheel', -600), ('content', 'Load metrics file'), ('wait_scroll_text', 'fixture-member'), ('capture', 'enterprise-users-light'),
        ('wheel', 1200), ('content', 'codex-fixture'), ('content', 'Refresh usage'),
        ('wait_text', 'Weekly allowance'), ('wait_text', 'Usage settings'), ('capture', 'codex-allowances-light'),
    ])
    # Both OCR engines read this font's "AI" prefix as "Al" on the actual
    # Mac capture. Require the complete fixture allowance and used count.
    # The footer links are Electron's usage-card links. The scene does not open
    # them, because a browser would leave the owned host.
    github.check(usage / 'usage-history-light.png', 'Credits: 1200 used / 3900 included', 'Projected period spend',
        'Open Copilot settings on GitHub', 'Copilot billing history')
    github.check(usage / 'enterprise-users-light.png', 'Enterprise users', 'fixture-member')
    github.check(usage / 'codex-allowances-light.png', 'Weekly allowance', '5-hour allowance', 'remaining', 'Usage settings')


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
    workspace_files = session_workspaces.prepare(user_directory, transcript)
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
                session_source_guard(transcript, transcript_sha, output), session_workspaces.source_guard(workspace_files, output):
            sdk = headless(executable, output, config, backend)
            assert_no_owned_children(output)
            terminal.verify_sentinel(sentinel, 'headless SDK cleanup')
            started_ms = time.time() * 1000
            with streaming_capture(output):
                chat_turns(driver)
            rerun_turns(driver)
            require_prompt_records(backend, started_ms)
            terminal.verify_sentinel(sentinel, 'chat app shutdown')
            assert_no_owned_children(output)
            code_copy(driver, 'dark')
            write_section(config, 'github')
            review_pull_request(driver)
            write_section(config, 'copilot')
            recent_results(driver)
            session_workspaces.qualify_dark(driver, user_directory, github.check)
            light = json.loads(fixture(output / 'light-template', 'light').read_text())['ui']
            write_section(config, 'github', light)
            api.publication_ack = False
            review_publication_unconfirmed(driver)
            publication = require_review_publications(api.requests, backend.query('copilotResults:listRecent'))
            (output / 'review-publication.json').write_text(json.dumps(publication, indent=2))
            write_section(config, 'copilot', light)
            usage_pages(driver)
            code_copy(driver, 'light')
            with environment(dict(BUDDY_CODEX_FIXTURE_SCENARIO='missing-auth')):
                missing = driver.drive('codex-auth-error-light', [('content', 'Usage'), ('content', 'codex-fixture'), ('content', 'Refresh usage'), ('wait_text', 'No Codex ChatGPT login found'), ('capture', 'codex-auth-error-light')])
                github.check(missing / 'codex-auth-error-light.png', 'No Codex ChatGPT login found')
            sessions = driver.drive('sessions-light', [
                ('content', 'Sessions'), ('input', 'VS Code User directory', str(user_directory), 'Scan folder'), ('content', 'Scan folder'),
                ('wait_text', 'Synthetic session'), ('content', 'Synthetic session'), ('wait_text', '100 prompt tokens'),
                ('capture', 'session-detail-light'), ('resize', 900, 650), ('capture', 'session-compact-light'),
                *session_workspaces.compact_actions(),
                ('wait_scroll_above', 'Inspect synthetic request 20', 'Next requests'),
                ('input', 'VS Code User directory', str(output / 'missing-directory'), 'Scan folder'), ('content', 'Scan folder'),
                ('wait_content', SESSION_SOURCE_ERROR), ('content', 'Next requests'),
                ('wait_scroll_above', 'Final synthetic request twenty one', 'Next requests'), ('capture', 'session-error-light'),
                ('content', 'Previous requests'), ('wait_scroll_above', 'Inspect synthetic request 20', 'Next requests'),
                ('wait_content', SESSION_SOURCE_ERROR),
            ])
            github.check(sessions / 'session-detail-light.png', '100 prompt tokens', '25 output tokens', '0.25')
            github.check(sessions / 'session-error-light.png', SESSION_SOURCE_ERROR, 'Loaded source')
            session_workspaces.check_light(sessions, github.check)
            terminal.verify_sentinel(sentinel, 'all native Copilot app shutdowns')
            assert_no_owned_children(output)
            (output / 'requests.json').write_text(json.dumps(api.requests, indent=2))
            assert_private_content_not_logged(output)
            startup = assert_startup_logs_clean(output)
            qualification = dict(result='passed', version=version, session=session,
                startupDiagnostics=startup,
                savedReviewPublication=publication,
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
