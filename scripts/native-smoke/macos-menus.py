"""Capture native menu pixels only on an owned disposable macOS CI desktop."""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import subprocess
import time

from desktop import UnixWindow, finish_cleanup, fixture, run

def load_script(name):
    spec = importlib.util.spec_from_file_location('menu_' + name.replace('-', '_'), Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ocr = load_script('settings-ocr')


CONTRACT = {
    'Buddy Native': {'About Buddy': None, 'Settings…': (',', 0), 'Services': None,
        'Hide Buddy Native': ('h', 0), 'Hide Others': ('h', 2), 'Show All': None,
        'Quit Buddy Native': ('q', 0)},
    'Edit': {'Undo': ('z', 0), 'Redo': ('z', 1), 'Cut': ('x', 0), 'Copy': ('c', 0),
        'Paste': ('v', 0), 'Select All': ('a', 0)},
    'Window': {'Minimize': ('m', 0), 'Zoom': None, 'Close Window': ('w', 0),
        'Bring All to Front': None},
}


def guard(output):
    temporary = os.environ.get('RUNNER_TEMP')
    if (platform.system() != 'Darwin' or os.environ.get('GITHUB_ACTIONS') != 'true'
            or os.environ.get('BUDDY_SETTINGS_FIXTURE') != '1' or not temporary
            or not output.is_relative_to(Path(temporary).resolve())
            or output == Path(temporary).resolve()):
        raise RuntimeError('Native menu capture requires an owned disposable Mac CI profile')


def rectangle(text):
    values = [float(value.strip()) for value in text.split(',')]
    if (len(values) != 4 or not all(math.isfinite(value) for value in values)
            or min(values[:2]) < 0 or not 50 <= values[2] <= 1000
            or not 40 <= values[3] <= 1600):
        raise RuntimeError('Invalid owned native menu bounds')
    x, y, width, height = values
    return [math.floor(x), math.floor(y), math.ceil(x + width) - math.floor(x),
            math.ceil(y + height) - math.floor(y)]


def verify(menu, rows, window_open=True):
    for label, accelerator in CONTRACT[menu].items():
        matches = [row for row in rows if row['label'] == label]
        if len(matches) != 1:
            raise RuntimeError(f'Native {menu} menu must contain exactly one {label}')
        # AppKit exposes lowercase native letter equivalents as uppercase AX
        # characters. Shift is represented independently in the modifier mask.
        actual = (matches[0]['key'].lower(), matches[0]['modifiers'])
        # Empty AppKit equivalents expose mask 0; absent AX attributes use -1.
        # Special-key accelerators may be represented by a glyph/virtual key.
        empty = (not actual[0] and actual[1] in (-1, 0)
            and matches[0].get('virtualKey') is None and matches[0].get('glyph') in (None, 0))
        if (accelerator is None and not empty) or (accelerator is not None and actual != accelerator):
            raise RuntimeError(f'Native accelerator mismatch for {label}: '
                f'expected {accelerator!r}, got raw key {matches[0]["key"]!r} '
                f'with modifiers {matches[0]["modifiers"]}, '
                f'virtual key {matches[0].get("virtualKey")}, glyph {matches[0].get("glyph")}')
        if menu == 'Window' and matches[0]['enabled'] != window_open:
            raise RuntimeError(f'Native window availability mismatch for {label}')


def menu_rows_script(menu):
    if menu not in CONTRACT:
        raise RuntimeError("Native menu requires a known menu")
    item = f'menu bar item {json.dumps(menu, ensure_ascii=False)} of menu bar 1'
    return f'''
set menuRows to ""
repeat with entry in menu items of menu 1 of {item}
    if name of entry is not missing value then
        set k to ""
        set m to -1
        set v to -1
        set g to -1
        try
            set k to value of attribute "AXMenuItemCmdChar" of entry
            set m to value of attribute "AXMenuItemCmdModifiers" of entry
        end try
        try
            set v to value of attribute "AXMenuItemCmdVirtualKey" of entry
        end try
        try
            set g to value of attribute "AXMenuItemCmdGlyph" of entry
        end try
        if k is missing value then set k to ""
        if m is missing value then set m to -1
        if v is missing value then set v to -1
        if g is missing value then set g to -1
        set menuRows to menuRows & (name of entry) & tab & k & tab & m & tab & (enabled of entry) & tab & v & tab & g & linefeed
    end if
end repeat
return menuRows
'''


# Failure-only, read-only state of the owned PID. Each attribute falls back to
# a fixed sentinel independently; the script sends no input or delay command.
FAILURE_STATE_SCRIPT = '''
set frontState to "unavailable"
set barCount to -1
set windowCount to -1
try
    set frontState to frontmost
end try
try
    set barCount to count of menu bars
end try
try
    set windowCount to count of windows
end try
return (frontState as text) & tab & barCount & tab & windowCount
'''
FAILURE_STATE_TIMEOUT = 1
FAILURE_ERROR_TYPES = ('RuntimeError', 'ValueError', 'OSError', 'TimeoutExpired')


def unavailable():
    return dict(state='unavailable')


def failure_count(text):
    # AppleScript renders integers as plain digits; the -1 sentinel and any
    # other shape stay unavailable rather than becoming a guessed value.
    if len(text) <= 3 and text.isascii() and text.isdigit():
        return dict(state='available', value=int(text))
    return unavailable()


def failure_process(process):
    try:
        code = process.poll()
    except Exception:
        return dict(state='unavailable', exitCode=None)
    if code is None:
        return dict(state='running', exitCode=None)
    if type(code) is int:
        return dict(state='exited', exitCode=code if -255 <= code <= 255 else None)
    return dict(state='unavailable', exitCode=None)


def failure_observation(window, running):
    fields = dict(frontmost=unavailable(), menuBarCount=unavailable(), windowCount=unavailable())
    if not running:
        return dict(status='not-run-process-not-running', **fields)
    try:
        output = run('osascript', '-e', window.apple(FAILURE_STATE_SCRIPT), timeout=FAILURE_STATE_TIMEOUT)
    except subprocess.TimeoutExpired:
        return dict(status='query-timeout', **fields)
    except Exception:
        return dict(status='query-failed', **fields)
    parts = output.split('\t') if isinstance(output, str) else []
    if len(parts) != 3:
        return dict(status='malformed', **fields)
    front, bars, windows = parts
    if front in ('true', 'false'):
        fields['frontmost'] = dict(state='available', value=front == 'true')
    elif front != 'unavailable':
        return dict(status='malformed', **fields)
    fields.update(menuBarCount=failure_count(bars), windowCount=failure_count(windows))
    return dict(status='observed', **fields)


def record_failure(window, menu, destination, stage, error):
    """Write fixed-schema owned state after a post-click failure. It never
    raises: the capture's own exception remains the reported failure."""
    try:
        process = failure_process(window.process)
        observation = failure_observation(window, process['state'] == 'running')
        name = type(error).__name__
        snapshot = dict(schema='buddy-native-menu-failure/v1', menu=menu, stage=stage,
            errorType=name if name in FAILURE_ERROR_TYPES else 'other',
            process=process, observation=observation)
        destination.with_name(destination.stem + '.failure.json').write_text(
            json.dumps(snapshot, indent=2))
    except Exception:
        pass


def capture(window, menu, destination, window_open=True):
    # Only known application menu names enter AppleScript. Geometry and labels
    # come from this owned PID's popup, never another process or the desktop.
    if menu not in CONTRACT or window.process.poll() is not None:
        raise RuntimeError('Native menu requires the live owned app and a known menu')
    item = f'menu bar item {json.dumps(menu, ensure_ascii=False)} of menu bar 1'
    run('osascript', '-e', window.apple(f'set frontmost to true\nclick {item}'))
    primary = None
    stage = 'bounds'
    try:
        script = f'''
set popup to menu 1 of {item}
set p to position of popup
set s to size of popup
return {{item 1 of p, item 2 of p, item 1 of s, item 2 of s}}
'''
        bounds = rectangle(run('osascript', '-e', window.apple(script)))
        stage = 'rows'
        script = menu_rows_script(menu)
        rows = []
        for line in run('osascript', '-e', window.apple(script)).splitlines():
            label, key, modifiers, enabled, virtual, glyph = line.split('\t')
            if enabled not in ('true', 'false'):
                raise RuntimeError('Native menu availability must be explicit')
            rows.append(dict(label=label, key=key, modifiers=int(modifiers), enabled=enabled == 'true',
                virtualKey=None if virtual == '-1' else int(virtual),
                glyph=None if glyph == '-1' else int(glyph)))
        stage = 'pixels'
        time.sleep(.3)
        run('screencapture', '-x', '-R', ','.join(map(str, bounds)), destination)
        if not destination.read_bytes().startswith(b'\x89PNG\r\n\x1a\n'):
            raise RuntimeError('Native menu pixels were not captured')
        stage = 'receipt'
        receipt = dict(menu=menu, bounds=bounds, items=rows,
            sha256=hashlib.sha256(destination.read_bytes()).hexdigest())
        # Preserve the actual pixels and unnormalized AX values even when the
        # contract fails, so the next investigation can distinguish app/driver.
        destination.with_suffix('.json').write_text(json.dumps(receipt, indent=2))
        stage = 'verify'
        verify(menu, rows, window_open)
        return receipt
    except Exception as error:
        primary = error
        # Observed after the failure and before Escape: one bounded read of
        # this PID, never a retry, a fix or a claim about the earlier routing.
        record_failure(window, menu, destination, stage, error)
        raise
    finally:
        # A required Escape failure fails a good capture, but cannot replace
        # the capture's own error.
        finish_cleanup(lambda: window.key('Escape', control=False), primary, 'Native menu Escape')


def wait_window(window, expected):
    deadline = time.monotonic() + 30
    last_error = None
    while time.monotonic() < deadline:
        if window.process.poll() is not None:
            raise RuntimeError('Owned native app exited during menu qualification')
        try:
            count = int(run('osascript', '-e', window.apple('get count of windows')))
            if count == expected:
                return
        except (subprocess.SubprocessError, RuntimeError, ValueError) as error:
            # Launch Services can register the owned PID after Popen returns.
            last_error = error
        time.sleep(.2)
    raise RuntimeError('Native window lifecycle did not reach its expected state') from last_error


def open_settings(window):
    run('osascript', '-e', window.apple('click menu bar item "Buddy Native" of menu bar 1'))
    run('osascript', '-e', window.apple('click menu item "Settings…" of menu 1 of menu bar item "Buddy Native" of menu bar 1'))
    wait_window(window, 1)


def capture_settings(window, image):
    deadline = time.monotonic() + 15
    while True:
        if window.process.poll() is not None:
            raise RuntimeError('Owned native app exited before Settings rendered')
        window.screenshot(image)
        words = ocr.recognize_macos(image)
        image.with_suffix('.words.json').write_text(json.dumps(words, indent=2))
        try:
            # Prove the requested view, rather than merely any reopened window.
            ocr.find_label(words, 'Settings', exact_line=True)
            ocr.find_label(words, 'GitHub username', field=True)
            return
        except RuntimeError:
            if time.monotonic() >= deadline:
                raise
            # Window creation precedes the first rendered frame. Retry only
            # observation, never the Settings command or persistence attempt.
            time.sleep(.2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--source-sha', required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    guard(output)
    if output.exists():
        raise RuntimeError('Native menu evidence requires a fresh directory')
    executable = args.executable.resolve(strict=True)
    if run(executable, '--version') != f'Buddy Native {args.version}':
        raise RuntimeError('Native menu executable version mismatch')
    receipts = []
    for theme in ('dark', 'light'):
        directory = output / theme
        config = fixture(directory, theme)
        env = {key: value for key, value in os.environ.items()
            if not key.startswith(('GH_', 'GITHUB_', 'BUDDY_', 'OTEL_'))}
        env.update(BUDDY_CONFIG_PATH=str(config), BUDDY_CONVEX_URL='http://127.0.0.1:9',
            GH_CONFIG_DIR=str(directory / 'empty-gh'))
        with (directory / 'app.log').open('w') as log:
            process = subprocess.Popen([str(executable)], env=env, cwd=directory, stdout=log, stderr=log)
            window = UnixWindow(process, 'macos')
            try:
                wait_window(window, 1)
                for name in CONTRACT:
                    filename = name.lower().replace(' ', '-') + '.png'
                    receipts.append(dict(theme=theme, **capture(window, name, directory / filename)))
                window.key('w')
                wait_window(window, 0)
                receipts.append(dict(theme=theme, **capture(window, 'Window',
                    directory / 'window-no-window.png', window_open=False)))
                # A read failure must preserve the corrupt file and still
                # reopen a window, where Settings.save_error is visible.
                saved_config = config.read_text()
                config.write_text('{invalid synthetic fixture')
                open_settings(window)
                capture_settings(window, directory / 'settings-save-error.png')
                if config.read_text() != '{invalid synthetic fixture':
                    raise RuntimeError('Settings failure overwrote the unreadable profile')
                config.write_text(saved_config)
                # Use the same owned input/OCR driver as installed settings
                # scenes. A real local form save must keep Settings selected.
                scene = load_script('settings-desktop')
                # The offline loader disables local fields until it finishes.
                # Observe its known loopback failure before sending any input.
                scene.wait_visible(window, directory, 'Convex is unavailable', timeout=60, content=True)
                driver = scene.Driver(
                    executable, directory, 'macos', None, Path(__file__).resolve().parents[2])
                driver.interact(window, directory, 'field', ['GitHub enterprise slug', 'synthetic-parity'])
                driver.interact(window, directory, 'save', ['GitHub enterprise slug'])
                capture_settings(window, directory / 'settings-recovery-save.png')
                recovered = json.loads(config.read_text())
                if (recovered['native']['activeSection'] != 'settings'
                        or recovered['ui']['enterpriseSlug'] != 'synthetic-parity'):
                    raise RuntimeError('The recovery form save lost its field or requested Settings section')
                window.key('w')
                wait_window(window, 0)
                # The app menu remains usable with no windows. Settings must
                # create exactly one real window, rather than exit or duplicate.
                open_settings(window)
                capture_settings(window, directory / 'settings-reopened.png')
                window.key('q')
                if process.wait(timeout=20) != 0:
                    raise RuntimeError('Native Quit did not exit cleanly')
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
    (output / 'qualification.json').write_text(json.dumps(dict(result='passed', sourceSha=args.source_sha,
        version=args.version, executableSha256=hashlib.sha256(executable.read_bytes()).hexdigest(),
        menus=receipts, closeKeepsApplication=True, settingsReopensWindow=True,
        settingsSaveFailureReopensWindow=True, settingsSaveFailureSelectsSettings=True,
        settingsSelectionSurvivesRecoverySave=True), indent=2))


if __name__ == '__main__':
    main()
