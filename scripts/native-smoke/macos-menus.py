"""Capture native menu pixels only on an owned disposable macOS CI desktop."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import subprocess
import time

from desktop import UnixWindow, fixture, run


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
        if accelerator and (matches[0]['key'], matches[0]['modifiers']) != accelerator:
            raise RuntimeError(f'Native accelerator mismatch for {label}')
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
        try
            set k to value of attribute "AXMenuItemCmdChar" of entry
            set m to value of attribute "AXMenuItemCmdModifiers" of entry
        end try
        if k is missing value then set k to ""
        if m is missing value then set m to -1
        set menuRows to menuRows & (name of entry) & tab & k & tab & m & tab & (enabled of entry) & linefeed
    end if
end repeat
return menuRows
'''


def capture(window, menu, destination, window_open=True):
    # Only known application menu names enter AppleScript. Geometry and labels
    # come from this owned PID's popup, never another process or the desktop.
    if menu not in CONTRACT or window.process.poll() is not None:
        raise RuntimeError('Native menu requires the live owned app and a known menu')
    item = f'menu bar item {json.dumps(menu, ensure_ascii=False)} of menu bar 1'
    run('osascript', '-e', window.apple(f'set frontmost to true\nclick {item}'))
    try:
        script = f'''
set popup to menu 1 of {item}
set p to position of popup
set s to size of popup
return {{item 1 of p, item 2 of p, item 1 of s, item 2 of s}}
'''
        bounds = rectangle(run('osascript', '-e', window.apple(script)))
        script = menu_rows_script(menu)
        rows = []
        for line in run('osascript', '-e', window.apple(script)).splitlines():
            label, key, modifiers, enabled = line.split('\t')
            if enabled not in ('true', 'false'):
                raise RuntimeError('Native menu availability must be explicit')
            rows.append(dict(label=label, key=key, modifiers=int(modifiers), enabled=enabled == 'true'))
        verify(menu, rows, window_open)
        time.sleep(.3)
        run('screencapture', '-x', '-R', ','.join(map(str, bounds)), destination)
        if not destination.read_bytes().startswith(b'\x89PNG\r\n\x1a\n'):
            raise RuntimeError('Native menu pixels were not captured')
        return dict(menu=menu, bounds=bounds, items=rows,
            sha256=hashlib.sha256(destination.read_bytes()).hexdigest())
    finally:
        window.key('Escape', control=False)


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
                # The app menu remains usable with no windows. Settings must
                # create exactly one real window, rather than exit or duplicate.
                run('osascript', '-e', window.apple('click menu bar item "Buddy Native" of menu bar 1'))
                run('osascript', '-e', window.apple('click menu item "Settings…" of menu 1 of menu bar item "Buddy Native" of menu bar 1'))
                wait_window(window, 1)
                window.screenshot(directory / 'settings-reopened.png')
                window.key('q')
                if process.wait(timeout=20) != 0:
                    raise RuntimeError('Native Quit did not exit cleanly')
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
    (output / 'qualification.json').write_text(json.dumps(dict(result='passed', sourceSha=args.source_sha,
        version=args.version, executableSha256=hashlib.sha256(executable.read_bytes()).hexdigest(),
        menus=receipts, closeKeepsApplication=True, settingsReopensWindow=True), indent=2))


if __name__ == '__main__':
    main()
