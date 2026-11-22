"""Synthetic read-only session grouping qualification, called by the owned CI scene."""
from contextlib import contextmanager
import hashlib
import json
import os

DAY = 24 * 60 * 60

def prepare(user, original):
    """Add only fresh synthetic workspaces beneath the existing owned fixture."""
    stamp = original.stat().st_mtime_ns
    files = {}
    for key, name, records in (
            ('alpha', 'Alpha', [('alpha-new', 'Alpha recent session', 'ALPHAID956', 10),
                                # Ages in seconds. Electron's date headings are
                                # This Week from 1 to 7 days before local
                                # midnight and Older beyond, so these two
                                # headings hold at any time of day.
                                ('alpha-old', 'Alpha earlier session', 'ALPHAOLD956', 3 * DAY)]),
            ('beta', 'Beta', [('beta', 'Beta selected session', 'BETAID956', 20)]),
            ('gamma', 'Gamma', [('gamma', 'Gamma hidden session', 'GAMMAID956', 30 * DAY)])):
        workspace = user / 'workspaceStorage' / ('groups-' + key)
        sessions = workspace / 'chatSessions'
        sessions.mkdir(parents=True, exist_ok=False)
        metadata = workspace / 'workspace.json'
        metadata.write_text(json.dumps({'folder': 'file:///synthetic/' + name}), encoding='utf-8')
        files[metadata] = hashlib.sha256(metadata.read_bytes()).hexdigest()
        for identifier, title, prompt, age in records:
            path = sessions / (identifier + '.jsonl')
            lines = [dict(kind=0, v=dict(sessionId=identifier, creationDate=1792152000123,
                customTitle=title, requests=[dict(message=dict(text=prompt))])),
                dict(kind=1, k=['requests', 0, 'result'], v=dict(
                    metadata=dict(promptTokens=1, outputTokens=1), timings=dict(totalElapsed=1)))]
            path.write_text('\n'.join(json.dumps(line) for line in lines), encoding='utf-8')
            # Fixed ordering before the read-only guard starts. The original
            # Synthetic session remains in the newest/default-expanded project.
            os.utime(path, ns=(stamp - age * 1_000_000_000, stamp - age * 1_000_000_000))
            files[path] = hashlib.sha256(path.read_bytes()).hexdigest()
    for path in (original, original.parent.parent / 'workspace.json'):
        files[path] = hashlib.sha256(path.read_bytes()).hexdigest()
    return files


@contextmanager
def source_guard(files, output):
    primary = None
    try:
        yield
    except BaseException as failure:
        primary = failure
        raise
    finally:
        try:
            changed = [str(path.relative_to(output)) for path, expected in files.items()
                if not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != expected]
            (output / 'session-workspace-source.json').write_text(json.dumps(dict(
                files=len(files), changed=changed, result='failed' if changed else 'passed'), indent=2), encoding='utf-8')
            if changed:
                raise RuntimeError('Synthetic session workspace files changed during read-only qualification')
        except BaseException as failure:
            if primary is None:
                raise
            primary.add_note('Session workspace source verification failed: ' + type(failure).__name__)


def qualify_dark(driver, user, check):
    capture = driver.drive('session-workspaces-dark', [
        ('content', 'Sessions'), ('input', 'VS Code User directory', str(user), 'Scan folder'),
        ('content', 'Scan folder'), ('wait_text', 'Alpha recent session'),
        ('content', 'Beta selected session'), ('wait_content', 'BETAID956'),
        ('capture', 'workspaces-expanded-dark'), ('content', 'Collapse Alpha'),
        ('capture', 'workspaces-collapsed-dark'), ('content', 'Collapse Beta'),
        ('capture', 'selected-workspace-hidden-dark'), ('content', 'Expand Gamma'),
        ('content', 'Gamma hidden session'), ('wait_content', 'GAMMAID956'),
        ('capture', 'expanded-gamma-dark'),
    ], record=True)
    check(capture / 'workspaces-expanded-dark.png', 'Alpha recent session', 'This Week', 'Alpha earlier session', 'Collapse Beta')
    check(capture / 'workspaces-collapsed-dark.png', 'Expand Alpha', 'BETAID956',
        absent=('Alpha recent session', 'Alpha earlier session'))
    check(capture / 'selected-workspace-hidden-dark.png', 'Expand Beta', 'BETAID956')
    check(capture / 'expanded-gamma-dark.png', 'GAMMAID956', 'Collapse Gamma', 'Older')


def compact_actions():
    return [('content', 'Collapse Alpha'), ('capture', 'workspaces-collapsed-compact-light'),
            ('content', 'Expand Alpha'), ('capture', 'workspaces-expanded-compact-light')]


def check_light(capture, check):
    check(capture / 'workspaces-collapsed-compact-light.png', 'Expand Alpha', '100 prompt tokens',
        absent=('Alpha recent session', 'Alpha earlier session'))
    check(capture / 'workspaces-expanded-compact-light.png', 'Collapse Alpha', 'Alpha recent session')
    check(capture / 'session-error-light.png', 'Collapse Alpha', 'Loaded source')
