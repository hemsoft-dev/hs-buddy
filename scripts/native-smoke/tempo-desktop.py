"""Installed Tempo UI against owned loopback services on disposable CI only."""
import argparse
import base64
from contextlib import contextmanager
from copy import deepcopy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import struct
import sys
from urllib.parse import urlsplit

from desktop import finish_cleanup, fixture

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location('tempo_desktop_' + name.replace('-', '_'), HERE / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


tempo = load('tempo-fixture')
github = load('github-desktop')
settings = github.settings


class ScenarioFixture(tempo.TempoFixture):
    def __init__(self):
        super().__init__(tempo_origin='https://api.eu.tempo.io')
        self.records[tempo.FIRST_ID]['startDate'] = '2026-10-15'
        for offset in (48, 49, 50):
            self.records[tempo.FIRST_ID + offset]['startDate'] = '2026-10-13'
        template = deepcopy(self.records[tempo.FIRST_ID])
        identifier = tempo.FIRST_ID + 100
        template.update(tempoWorklogId=identifier, startDate='2026-09-30',
            timeSpentSeconds=1800, billableSeconds=0, description='Synthetic prior month template',
            attributes=dict(values=[dict(key='_Unrelated_', value='preserve\n界'), dict(key='_Unset_')]))
        template['self'] = f'{self.tempo_origin}/4/worklogs/{identifier}'
        template['attributes']['self'] = template['self'] + '/work-attribute-values'
        self.records[identifier] = template
        self.next_id = identifier + 1
        self.drop_after = None
        self.expire_after_lookup = False
        self.reject_identity = False
        self.deny_next_create = False
        self.rejections = []
        self.preflight_after_lookup = False
        self.preflight_identity_reads = None
        self.preflight_failures = []

    def arm_lost_reply_after(self, confirmed):
        if type(confirmed) is not int or confirmed < 0 or confirmed > 10:
            raise ValueError('Use a bounded number of confirmed synthetic creates')
        with self.lock:
            if self.drop_after is not None:
                raise RuntimeError('A lost-reply scenario is already armed')
            self.drop_after = confirmed

    def respond(self, method, target, authorization, raw):
        path = urlsplit(target).path
        if method == 'GET' and path == '/rest/api/3/myself' and self.preflight_identity_reads is not None:
            if self.preflight_identity_reads == 0:
                self.preflight_identity_reads = None
                mutations = sum(row['method'] in ('POST', 'PUT', 'DELETE') for row in self.attempts)
                self.preflight_failures.append(dict(writes=len(self.writes), mutationAttempts=mutations))
                return 503, dict(error='Synthetic dispatch checkpoint unavailable')
            self.preflight_identity_reads -= 1
        if (method == 'POST' and path == '/4/worklogs' and self.deny_next_create
                and authorization == 'Bearer ' + tempo.TEMPO_TOKEN):
            self.deny_next_create = False
            self.rejections.append(dict(method=method, path=target, bodyBase64=base64.b64encode(raw).decode('ascii')))
            return 403, dict(error='Synthetic worklog permission denied')
        if method == 'GET' and path == '/rest/api/3/myself' and self.reject_identity:
            self.reject_identity = False
            return 401, dict(error='Synthetic one-shot expired connection')
        status, value = super().respond(method, target, authorization, raw)
        if method == 'GET' and path == '/4/accounts' and status == 200 and self.preflight_after_lookup:
            self.preflight_after_lookup = False
            self.preflight_identity_reads = 2
        if method == 'GET' and path == '/4/accounts' and status == 200 and self.expire_after_lookup:
            self.expire_after_lookup = False
            self.reject_identity = True
        if method == 'POST' and status == 200 and self.drop_after is not None:
            if self.drop_after == 0:
                self.drop_after = None
                return None, None
            self.drop_after -= 1
        return status, value

    def snapshot(self):
        with self.lock:
            return deepcopy(dict(records=self.records, attempts=self.attempts, writes=self.writes, rejections=self.rejections, preflightFailures=self.preflight_failures))


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


def write_body(row):
    return json.loads(base64.b64decode(row['bodyBase64'], validate=True).decode('utf-8'))


def require_writes(api, count, *, rejected=0):
    state = api.snapshot()
    mutations = [row for row in state['attempts'] if row['method'] in ('POST', 'PUT', 'DELETE')]
    if (len(mutations) != count + rejected or len(state['writes']) != count
            or len(state['rejections']) != rejected):
        raise RuntimeError('Installed Tempo sent an unexpected number of mutation attempts')
    return state


def pending_bytes(config):
    paths = list(config.parent.glob('native-tempo-change-*.json'))
    if len(paths) != 1 or paths[0].is_symlink() or not paths[0].is_file():
        raise RuntimeError('Expected one original owned Tempo recovery record')
    return paths[0].read_bytes()


def assert_copy(api, original, expected_start):
    state = require_writes(api, 2, rejected=1)
    row = state['writes'][-1]
    body = write_body(row)
    expected = dict(authorAccountId=tempo.ACCOUNT, issueId=tempo.ISSUE_ID,
        startDate='2026-10-16', startTime=expected_start, timeSpentSeconds=3661,
        billableSeconds=3600, description=original['description'], attributes=original['attributes']['values'])
    if row['method'] != 'POST' or body != expected or state['records'][tempo.FIRST_ID] != original:
        raise RuntimeError('Copying changed source fields, duration, attributes or stacked start time')
    return state


class TempoDriver(settings.Driver):
    """Target the inner editor of every macOS Tempo input.

    Mac OCR label height depends on descenders: Intel measured "Jira email" at
    10pt, so the default 20pt offset below the label reached an editor's top
    border and typed nothing. Stacked editors start about 25pt below their
    labels, and the bounded 30pt offset reaches each inner editor. The From
    and Through editors start about 17pt below, so they keep the default.
    Other sessions keep their measured spacing.
    """

    def drive(self, name, actions, concurrent=False, record=False):
        if self.session == 'macos':
            actions = [action + ({'labelBottomOffset': 30},)
                if action[0] == 'input' and action[1] not in ('From', 'Through')
                and not isinstance(action[-1], dict) else action for action in actions]
        return super().drive(name, actions, concurrent, record)


def week_actions():
    # Date ranges belong to the open session. Every new process starts at the
    # account month, so choose the test week through the UI in each sequence.
    return [
        ('wait_text', 'Tempo timesheet'),
        ('wait_scroll_text', 'Load date range', 'Tempo timesheet'),
        ('input', 'From', '2026-10-12'), ('input', 'Through', '2026-10-18'),
        ('click', 'Load date range'), ('wait_text', '2026-10-12 to 2026-10-18'),
        ('click', 'Timesheet'),
    ]


def evidence_inventory(output):
    required = {
        'connect-dark/missing-connection-dark.png', 'connect-dark/regional-connection-dark.png',
        'connect-dark/complete-month-dark.png', 'range-navigation-dark/custom-range-retained-dark.png',
        'grid-navigation-dark/compact-grid-keyboard-dark.png',
        'grid-navigation-dark/filtered-cell-dark.png', 'grid-navigation-dark/second-page-dark.png',
        'template-dark/template-new-draft-dark.png', 'template-dark/template-created-dark.png',
        'template-dark/unsent-captured-change-dark.png',
        'denied-write-dark/invalid-duration-dark.png', 'denied-write-dark/permission-denied-dark.png',
        'denied-write-dark/denied-hours-retained-dark.png', 'denied-write-dark/denied-description-retained-dark.png',
        'copy-dark/copy-confirmation-dark.png', 'copy-dark/copy-confirmed-dark.png',
        'move-dark/exact-original-editor-dark.png', 'move-dark/moved-worklog-dark.png',
        'reauth-draft-dark/expired-open-draft-dark.png', 'reauth-draft-dark/draft-needs-fresh-lookup-dark.png',
        'reauth-draft-dark/retained-draft-after-lookup-dark.png',
        'reauth-draft-dark/retained-hours-after-lookup-dark.png',
        'partial-copy-dark/three-copy-confirmation-dark.png', 'partial-copy-dark/partial-copy-unconfirmed-dark.png',
        'recovery-light/restarted-recovery-light.png', 'recovery-light/compact-keyboard-recovery-light.png',
        'discard-recovery-light/discard-confirmation-light.png', 'discard-recovery-light/discarded-local-only-light.png',
        'delete-light/delete-confirmation-light.png', 'delete-light/deleted-empty-range-light.png',
        'forget-light/connection-removed-light.png', 'forgotten-restart-light/missing-after-restart-light.png',
    }
    screenshots = []
    for name in sorted(required):
        path = output / name
        if path.is_symlink() or not path.is_file():
            raise RuntimeError('Missing an original installed Tempo state capture: ' + name)
        data = path.read_bytes()
        if len(data) < 24 or data[:8] != b'\x89PNG\r\n\x1a\n' or data[12:16] != b'IHDR':
            raise RuntimeError('Invalid original Tempo capture')
        width, height = struct.unpack('>II', data[16:24])
        if not (width >= 800 and height >= 600):
            raise RuntimeError('The original Tempo capture is below its supported dimensions')
        screenshots.append(dict(path=name, width=width, height=height, sha256=hashlib.sha256(data).hexdigest()))
    recording = output / 'copy-dark/interaction.mp4'
    if recording.is_symlink() or not recording.is_file() or recording.stat().st_size == 0:
        raise RuntimeError('Missing the installed Tempo interaction recording')
    probe = json.loads(settings.run('ffprobe', '-v', 'error', '-show_streams', '-show_format', '-of', 'json', recording))
    if (any(stream['codec_type'] == 'audio' for stream in probe['streams'])
            or not any(stream['codec_type'] == 'video' for stream in probe['streams'])
            or float(probe['format']['duration']) <= 0):
        raise RuntimeError('The Tempo recording needs a video stream and no audio')
    frames = sorted((output / 'copy-dark/frames').glob('frame-*.png'))
    if len(frames) < 10 or any(path.is_symlink() for path in frames):
        raise RuntimeError('Missing the original unresized Tempo recording frames')
    return dict(screenshots=screenshots, recording=dict(path=recording.relative_to(output).as_posix(),
        sha256=hashlib.sha256(recording.read_bytes()).hexdigest(), duration=probe['format']['duration'],
        originalFrameCount=len(frames), silent=True))


def qualify(executable, output, session, version, source, backend):
    if not re.fullmatch('[0-9a-f]{40}', source):
        raise RuntimeError('Missing verified package source provenance')
    binary = github.installed_binary(executable, version)
    config = fixture(output, 'dark')
    value = json.loads(config.read_text())
    value['native']['activeSection'] = 'tempo'
    config.write_text(json.dumps(value), encoding='utf-8')
    driver = TempoDriver(executable, output, session, backend, HERE.parent.parent)
    marker = config.parent / '.buddy-tempo-fixture.json'
    if marker.exists():
        raise RuntimeError('Tempo qualification needs a fresh owned marker')
    with ScenarioFixture() as api:
        marker.write_text(json.dumps(api.marker()), encoding='utf-8')
        primary = None
        try:
            with environment(dict(BUDDY_TEMPO_FIXTURE='1', BUDDY_CONFIG_PATH=config,
                    BUDDY_CONVEX_URL=backend.url, RUST_LOG='warn,reqwest=trace,hyper=trace')):
                initial = api.snapshot()
                original = initial['records'][tempo.FIRST_ID]
                (output / 'initial-state.json').write_text(json.dumps(initial, indent=2), encoding='utf-8')
                connected = driver.drive('connect-dark', [
                    ('wait_text', 'Tempo and Jira connection'), ('capture', 'missing-connection-dark'),
                    ('click', 'Europe'), ('wait_text', 'https://api.eu.tempo.io'),
                    ('capture', 'regional-connection-dark'),
                    ('input', 'Tempo API token', tempo.TEMPO_TOKEN),
                    ('input', 'Jira HTTPS origin', tempo.JIRA_ORIGIN),
                    ('input', 'Jira email', tempo.JIRA_EMAIL),
                    ('input', 'Jira API token', tempo.JIRA_TOKEN),
                    ('wait_scroll_text', 'Save and connect', 'Jira API token'),
                    ('click', 'Save and connect'), ('wait_text', 'America/New_York'),
                    ('capture', 'complete-month-dark'),
                ])
                github.check(connected / 'complete-month-dark.png', 'Tempo timesheet', 'Logged 1:51:01', 'CapEx 1:51:01', 'Required 168:00')
                custom = driver.drive('range-navigation-dark', [
                    ('wait_scroll_text', 'Load date range', 'Tempo timesheet'),
                    ('input', 'From', '2026-10-05'), ('input', 'Through', '2026-10-19'),
                    ('click', 'Load date range'), ('wait_text', 'Date range loaded'),
                    ('click', 'Previous month'), ('wait_scroll_up', 'whole month'),
                    ('capture', 'custom-range-retained-dark'),
                    ('click', 'Today'), ('wait_text', 'Loaded the current account month'),
                ])
                github.check(custom / 'custom-range-retained-dark.png', '2026-10-05', '2026-10-19', 'whole month')
                require_writes(api, 0)
                pages = [row for row in api.snapshot()['attempts'] if '/worklogs/user/' in row['path'] and 'offset=50' in row['path']]
                if not pages:
                    raise RuntimeError('The installed app did not read the second worklog page')
                navigation = driver.drive('grid-navigation-dark', week_actions() + [
                    ('wait_scroll_text', 'FIX-17', 'Load date range'),
                    ('resize', 900, 650), ('key', 'Tab'), ('capture', 'compact-grid-keyboard-dark'),
                    ('resize', 1200, 820), ('cell', 'FIX-17', '16 Oct'),
                    ('wait_text', 'Worklogs for FIX-17 on 2026-10-16'),
                    ('wait_scroll_text', 'Show all worklogs', 'Load date range'),
                    ('capture', 'filtered-cell-dark'), ('click', 'Show all worklogs'),
                    ('wheel', -7200),
                    ('wait_scroll_text', 'Next rows', 'Load date range'), ('click', 'Next rows'),
                    ('wait_text', 'page 2'), ('capture', 'second-page-dark'),
                ])
                if require_writes(api, 0)['records'] != initial['records']:
                    raise RuntimeError('Read-only grid and filtered Timeline navigation changed worklogs')
                if json.loads(config.read_text())['native'].get('tempoViewMode') != 'grid':
                    raise RuntimeError('Temporary cell filtering changed the saved explicit grid preference')
                github.check(navigation / 'filtered-cell-dark.png', 'Worklogs for FIX-17', '2026-10-16', 'Show all worklogs')
                with api.lock:
                    api.preflight_after_lookup = True
                templates = driver.drive('template-dark', week_actions() + [
                    ('capture', 'week-grid-dark'), ('click', "Last month's issues"),
                    ('wait_text', 'Loaded 1 issue templates'),
                    ('wait_scroll_text', 'FIX-17', 'Load date range'),
                    ('capture', 'prior-month-issues-dark'), ('cell', 'FIX-17', '14 Oct'),
                    ('wait_scroll_up', 'Hours or H:MM:SS'),
                    ('wait_scroll_text', 'Synthetic prior month template', 'Hours or H:MM:SS'),
                    ('wait_scroll_text', 'No billing account', 'Description'),
                    ('capture', 'template-new-draft-dark'),
                    ('wait_scroll_text', 'Create worklog', 'Description'), ('click', 'Create worklog'),
                    ('wait_scroll_up', 'This captured change has not been sent'),
                    ('capture', 'unsent-captured-change-dark'), ('click', 'Retry captured change'),
                    ('wait_scroll_up', 'Tempo confirmed the worklog change'), ('capture', 'template-created-dark'),
                ])
                state = require_writes(api, 1)
                if state['preflightFailures'] != [dict(writes=0, mutationAttempts=0)]:
                    raise RuntimeError('The explicit captured retry did not follow exactly one unsent preflight failure')
                body = write_body(state['writes'][0])
                expected_template = dict(authorAccountId=tempo.ACCOUNT, issueId=tempo.ISSUE_ID,
                    timeSpentSeconds=3600, startDate='2026-10-14', startTime='08:00:00',
                    description='Synthetic prior month template', attributes=[])
                if state['writes'][0]['method'] != 'POST' or body != expected_template:
                    raise RuntimeError('The template did not create one hour with explicit no billing')
                github.check(templates / 'template-new-draft-dark.png', 'Create worklog', 'No billing account', 'Synthetic prior month template')
                before_denied = state['records']
                with api.lock:
                    api.deny_next_create = True
                denied = driver.drive('denied-write-dark', [
                    ('wait_text', 'Tempo timesheet'), ('click', 'Log time'),
                    ('wait_scroll_text', 'Issue key or ID', 'Tempo timesheet'),
                    ('input', 'Issue key or ID', 'FIX-17'),
                    ('click', 'Look up issue and billing accounts'), ('wait_text', 'Capitalization: CapEx'),
                    ('input', 'Hours or H:MM:SS', '0'), ('input', 'Work date', '2026-10-14'),
                    ('input', 'Start time in the account timezone', '09:30:01'),
                    ('wait_scroll_text', 'Description', 'Hours or H:MM:SS'),
                    ('reveal_text', 'Work description', 'Description'),
                    ('input', 'Description', 'Denied native draft 界'),
                    ('wait_scroll_text', 'No billing account', 'Description'), ('click', 'No billing account'),
                    ('wait_scroll_text', 'Create worklog', 'No billing account'), ('click', 'Create worklog'),
                    ('wait_scroll_up', 'Enter positive hours'), ('capture', 'invalid-duration-dark'),
                    ('wait_scroll_text', 'Hours or H:MM:SS', 'Tempo timesheet'),
                    ('input', 'Hours or H:MM:SS', '1:30:01'),
                    ('wait_scroll_text', 'Create worklog', 'Hours or H:MM:SS'), ('click', 'Create worklog'),
                    ('wait_scroll_up', 'Tempo rejected the worklog change'), ('capture', 'permission-denied-dark'),
                    # OCR cannot read a value inside the focus ring; move focus
                    # to the next editor so the retained hours are legible.
                    ('key', 'Tab'),
                    ('wait_scroll_text', 'Hours or H:MM:SS', 'Tempo timesheet'), ('capture', 'denied-hours-retained-dark'),
                    # A visible label does not mean its editor is: reveal the
                    # retained value in fine steps before the capture.
                    ('wait_scroll_text', 'Description', 'Hours or H:MM:SS'),
                    ('reveal_text', 'Denied native draft', 'Description'), ('capture', 'denied-description-retained-dark'),
                    ('wait_scroll_text', 'Cancel editor', 'Description'), ('click', 'Cancel editor'),
                ])
                state = require_writes(api, 1, rejected=1)
                rejected_body = write_body(state['rejections'][0])
                expected_rejected = dict(authorAccountId=tempo.ACCOUNT, issueId=tempo.ISSUE_ID,
                    timeSpentSeconds=5401, startDate='2026-10-14', startTime='09:30:01',
                    description='Denied native draft 界', attributes=[])
                if rejected_body != expected_rejected or state['records'] != before_denied or api.deny_next_create:
                    raise RuntimeError('The invalid/denied draft changed worklogs or sent an unexpected request')
                github.check(denied / 'invalid-duration-dark.png', 'Enter positive hours')
                github.check(denied / 'permission-denied-dark.png', 'Tempo rejected the worklog change', 'draft was retained')
                github.check(denied / 'denied-hours-retained-dark.png', 'Hours or H:MM:SS', '1:30:01')
                github.check(denied / 'denied-description-retained-dark.png', 'Description', 'Denied native draft')
                copied = driver.drive('copy-dark', week_actions() + [
                    ('wait_scroll_text', 'Copy day to Today', 'Load date range'),
                    ('cell', 'Copy day to Today', '15 Oct'),
                    ('wait_scroll_up', 'Copy worklogs to Today'), ('capture', 'copy-confirmation-dark'),
                    ('wait_scroll_text', 'Create copies', 'Copy worklogs to Today'),
                    ('click', 'Create copies'), ('wait_scroll_up', '1 worklog copied to Today'),
                    ('capture', 'copy-confirmed-dark'),
                ], record=True)
                assert_copy(api, original, '08:47:00')
                # Each drive starts a new real process and reloads the saved
                # account from the isolated OS vault. No test restores state.
                edited = driver.drive('move-dark', week_actions() + [
                    ('wait_scroll_text', 'FIX-17', 'Load date range'), ('cell', 'FIX-17', '15 Oct'),
                    ('wait_scroll_up', 'Hours or H:MM:SS'), ('capture', 'exact-original-editor-dark'),
                    ('input', 'Work date', '2026-11-05'),
                    ('wait_scroll_text', 'Update worklog', 'Work date'), ('click', 'Update worklog'),
                    ('wait_scroll_up', '2026-11-01 to 2026-11-30'), ('click', 'Timeline'),
                    ('wait_scroll_text', 'Synthetic worklog', 'Load date range'),
                    ('capture', 'moved-worklog-dark'),
                ])
                state = require_writes(api, 3, rejected=1)
                moved = state['records'][tempo.FIRST_ID]
                if moved != {**original, 'startDate': '2026-11-05'}:
                    raise RuntimeError('Moving the work date did not preserve every original worklog field')
                github.check(edited / 'moved-worklog-dark.png', '05 Nov 2026', '1:01:01', 'Synthetic worklog')
                with api.lock:
                    api.expire_after_lookup = True
                rechecked = driver.drive('reauth-draft-dark', [
                    ('wait_text', 'Tempo timesheet'), ('click', 'Log time'),
                    ('wait_scroll_text', 'Issue key or ID', 'Tempo timesheet'),
                    ('input', 'Issue key or ID', 'FIX-17'),
                    ('click', 'Look up issue and billing accounts'), ('wait_text', 'Capitalization: CapEx'),
                    # Description is a multi-line textarea where Tab does not
                    # move focus. Type it first, then leave the Hours editor
                    # with Tab so OCR can read both retained values.
                    ('wait_scroll_text', 'Description', 'Capitalization: CapEx'),
                    ('reveal_text', 'Work description', 'Description'),
                    ('input', 'Description', 'Retained native draft 界'),
                    ('wait_scroll_up', 'Hours or H:MM:SS', 'Description'),
                    ('input', 'Hours or H:MM:SS', '1:01:01'), ('key', 'Tab'),
                    ('wait_scroll_up', 'Refresh', 'Hours or H:MM:SS'),
                    ('click', 'Refresh'), ('wait_text', 'Jira rejected this connection'),
                    ('capture', 'expired-open-draft-dark'),
                    ('click', 'Connection'), ('wait_scroll_text', 'Recheck saved connection', 'Tempo timesheet'),
                    # The connection panel closes after a recheck, so the view
                    # can land in the worklog list below the status.
                    ('click', 'Recheck saved connection'), ('wait_scroll_up', 'Your draft was retained'),
                    ('wait_scroll_text', 'Look up issue and billing accounts', 'Tempo timesheet'),
                    ('capture', 'draft-needs-fresh-lookup-dark'),
                    ('click', 'Look up issue and billing accounts'), ('wait_text', 'Capitalization: CapEx'),
                    ('wait_scroll_text', 'Hours or H:MM:SS', 'Look up issue and billing accounts'),
                    ('reveal_text', 'Work date', 'Hours or H:MM:SS'),
                    ('capture', 'retained-hours-after-lookup-dark'),
                    # Short anchored hops: one coarse scroll from Hours can
                    # pass the whole draft above the Timeline list.
                    ('wait_scroll_text', 'Description', 'Hours or H:MM:SS'),
                    ('reveal_text', 'No billing account', 'Description'),
                    ('capture', 'retained-draft-after-lookup-dark'),
                    # Cancel editor sits just below the fold; a coarse Mac
                    # scroll jumped past it into the Timeline list.
                    ('reveal_text', 'Cancel editor', 'No billing account'), ('click', 'Cancel editor'),
                ])
                if require_writes(api, 3, rejected=1)['records'] != state['records']:
                    raise RuntimeError('Read-only draft reauthentication changed remote worklogs')
                if api.expire_after_lookup or api.reject_identity:
                    raise RuntimeError('The installed reauthentication did not consume its one-shot expiry')
                github.check(rechecked / 'retained-hours-after-lookup-dark.png', 'Hours or H:MM:SS', '1:01:01')
                github.check(rechecked / 'retained-draft-after-lookup-dark.png', 'Retained native draft', 'No billing account')

                api.arm_lost_reply_after(1)
                partial = driver.drive('partial-copy-dark', week_actions() + [
                    ('wait_scroll_text', 'Copy day to Today', 'Load date range'),
                    ('cell', 'Copy day to Today', '13 Oct'), ('wait_scroll_up', 'Copy worklogs to Today'),
                    ('capture', 'three-copy-confirmation-dark'),
                    ('wait_scroll_text', 'Create copies', 'Copy worklogs to Today'), ('click', 'Create copies'),
                    # About 24 one-second-paced requests precede the outcome.
                    ('sleep', 30), ('wait_scroll_up', '1 worklog copied to Today'),
                    ('capture', 'partial-copy-unconfirmed-dark'),
                ])
                state = require_writes(api, 5, rejected=1)
                bodies = [write_body(row) for row in state['writes'][-2:]]
                for body, start in zip(bodies, ('09:48:01', '09:49:01')):
                    if body['startDate'] != '2026-10-16' or body['timeSpentSeconds'] != 60 or body['startTime'] != start:
                        raise RuntimeError('The partial batch did not use exact complete Today totals')
                if [body['description'] for body in bodies] != ['Synthetic page row 48', 'Synthetic page row 49']:
                    raise RuntimeError('The batch copied an unexpected source or attempted its third worklog')
                github.check(partial / 'partial-copy-unconfirmed-dark.png', '1 worklog copied', 'unconfirmed result', '1 worklog was not attempted')
                original_recovery = pending_bytes(config)
                (output / 'original-recovery.bin').write_bytes(original_recovery)

                light = json.loads(fixture(output / 'light-template', 'light').read_text())['ui']
                value = json.loads(config.read_text())
                value['ui'].update(light)
                config.write_text(json.dumps(value), encoding='utf-8')
                recovered = driver.drive('recovery-light', [
                    ('wait_text', 'will not resend automatically'), ('capture', 'restarted-recovery-light'),
                    ('click', 'Refresh'), ('wait_text', 'Timesheet refreshed'),
                    ('resize', 900, 650), ('key', 'Tab'), ('capture', 'compact-keyboard-recovery-light'),
                ])
                if pending_bytes(config) != original_recovery:
                    raise RuntimeError('Restart or refresh changed the immutable recovery record')
                require_writes(api, 5, rejected=1)
                github.check(recovered / 'restarted-recovery-light.png', 'may already exist in Tempo', 'will not resend automatically')
                driver.drive('discard-recovery-light', [
                    ('wait_text', 'will not resend automatically'), ('click', 'Discard local recovery'),
                    ('wait_text', 'Keep recovery'), ('capture', 'discard-confirmation-light'), ('click', 'Keep recovery'),
                    ('click', 'Discard local recovery'), ('click', 'Confirm discard'),
                    ('wait_text', 'Local recovery discarded'), ('capture', 'discarded-local-only-light'),
                ])
                require_writes(api, 5, rejected=1)
                if list(config.parent.glob('native-tempo-change-*.json')):
                    raise RuntimeError('Explicit discard left a local Tempo recovery record')
                # A one-day range isolates the moved source, so visible action
                # labels cannot select another worklog by discovery order.
                deleted = driver.drive('delete-light', [
                    ('wait_text', 'Tempo timesheet'), ('wait_scroll_text', 'Load date range', 'Tempo timesheet'),
                    ('input', 'From', '2026-11-05'), ('input', 'Through', '2026-11-05'),
                    ('click', 'Load date range'), ('wait_text', '2026-11-05 to 2026-11-05'),
                    ('click', 'Timeline'), ('wait_scroll_text', 'Delete worklog', 'Load date range'),
                    ('click', 'Delete worklog'), ('wait_scroll_up', 'Confirm deletion'),
                    ('capture', 'delete-confirmation-light'), ('click', 'Confirm deletion'),
                    ('wait_scroll_up', 'Tempo confirmed the worklog change'),
                    ('wait_scroll_text', 'No worklogs in this date range', 'Load date range'),
                    ('capture', 'deleted-empty-range-light'),
                ])
                state = require_writes(api, 6, rejected=1)
                if tempo.FIRST_ID in state['records'] or state['writes'][-1]['method'] != 'DELETE':
                    raise RuntimeError('The explicit deletion did not remove exactly its captured worklog')
                github.check(deleted / 'deleted-empty-range-light.png', 'No worklogs in this date range')
                preference = json.loads(config.read_text())
                if preference['native'].get('tempoViewMode') != 'timeline' or preference.get('packageQualificationSentinel') != {'preserve': True}:
                    raise RuntimeError('Saving the view preference changed unrelated shared config')
                before_forget = api.snapshot()['records']
                driver.drive('forget-light', [
                    ('wait_text', 'Tempo timesheet'), ('click', 'Connection'),
                    ('wait_scroll_text', 'Forget saved connection', 'Tempo timesheet'),
                    # The status renders at the top, above the scrolled panel.
                    ('click', 'Forget saved connection'), ('wait_scroll_up', 'credentials were removed'),
                    ('capture', 'connection-removed-light'),
                ])
                driver.drive('forgotten-restart-light', [
                    ('wait_text', 'No connection is saved for this profile'),
                    ('capture', 'missing-after-restart-light'),
                ])
                state = require_writes(api, 6, rejected=1)
                if state['records'] != before_forget:
                    raise RuntimeError('Credential removal changed the remote worklogs')
                for path in output.glob('*/app.log'):
                    text = path.read_text(encoding='utf-8', errors='replace')
                    if any(token in text for token in (tempo.TEMPO_TOKEN, tempo.JIRA_TOKEN)):
                        raise RuntimeError('A synthetic credential appeared in application tracing')
                (output / 'requests.json').write_text(json.dumps(api.snapshot(), indent=2), encoding='utf-8')
                inventory = evidence_inventory(output)
                receipt = dict(result='passed', version=version, session=session, sourceSha=source, **binary,
                    largeWindow=driver.large_size, compactWindow=[900, 650], **inventory,
                    mutationAttempts=7, serviceWrites=6, acknowledgedServiceWrites=5, deniedMutationAttempts=1,
                    invalidDurationBlockedBeforeHttp=True, deniedCreateRetainedExactDraft=True,
                    completeMonthPagerVerified=True,
                    tempoApiOrigin=api.tempo_origin, regionalConnectionSurvivedRestart=True,
                    customRangeRetainedOnMonthNavigation=True,
                    readOnlyNavigationServiceStateUnchanged=True,
                    gridSingleEditAndMultiLogTimelineVerified=True,
                    templateCreatesOneHourWithExplicitNoBilling=True,
                    unsentCaptureExplicitRetryCreatesExactlyOnce=True,
                    copyPreservesExactSecondsBillableAttributesAndSource=True,
                    movedEditLoadsItsMonthAndPreservesOriginalFields=True,
                    draftReauthenticationRetainsInputsAndRequiresFreshLookup=True,
                    partialBatchConfirmed=1, partialBatchUnconfirmed=1, partialBatchUnattempted=1,
                    originalRecoverySha256=hashlib.sha256(original_recovery).hexdigest(),
                    restartAndRefreshNeverReplay=True, explicitDiscardChangesOnlyLocalRecovery=True,
                    explicitDeletionVerified=True, viewPreferenceAndUnknownConfigFieldsPreserved=True,
                    realOsCredentialStore=True, secureConnectionSurvivesProcessRestart=True,
                    secureConnectionRemovalVerifiedAfterRestart=True,
                    dataSource='Owned numeric-loopback Tempo/Jira fixture and isolated anonymous Convex')
        except BaseException as failure:
            primary = failure
            try:
                (output / 'failed-requests.json').write_text(json.dumps(api.snapshot(), indent=2), encoding='utf-8')
            except BaseException as diagnostic:
                failure.add_note('Tempo request diagnostics failed: ' + type(diagnostic).__name__)
            raise
        finally:
            finish_cleanup(marker.unlink, primary, 'Tempo fixture marker removal')
    (output / 'qualification.json').write_text(json.dumps(receipt, indent=2), encoding='utf-8')


def main():
    if os.environ.get('GITHUB_ACTIONS') != 'true' or os.environ.get('BUDDY_SETTINGS_FIXTURE') != '1':
        raise RuntimeError('Tempo qualification requires an owned disposable GitHub Actions desktop')
    parser = argparse.ArgumentParser()
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--session', choices=('windows', 'macos', 'x11'), required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--source-sha', required=True)
    args = parser.parse_args()
    output = load('terminal-desktop').owned_output(args.output)
    with settings.SettingsBackend(HERE.parent.parent) as backend:
        qualify(args.executable.resolve(strict=True), output, args.session, args.version, args.source_sha, backend)


if __name__ == '__main__':
    main()
