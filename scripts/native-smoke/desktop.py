"""Exercise an installed Buddy package, using only an isolated CI desktop.

Windows uses a never-shown desktop and posted messages. Unix desktop drivers
require GitHub Actions so they cannot send input to a developer's desktop.
Screenshots contain real OS window pixels; OCR must find dashboard content.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import struct
import sys
import time
from threading import Event


HERE = Path(__file__).resolve().parent


def run(*args, **kwargs):
    timeout = kwargs.pop("timeout", 60)
    try:
        return subprocess.run(
            [str(arg) for arg in args], check=True, capture_output=True,
            text=True, timeout=timeout, **kwargs,
        ).stdout.strip()
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"{args[0]} failed with exit {error.returncode}: {error.stderr[-8000:]}") from error


def require_version(text, version):
    if not re.search(rf"(?<![\w.]){re.escape(version)}(?![\w.])", text, re.IGNORECASE):
        raise RuntimeError(f"Screenshot is missing exact version {version!r}")


def require_dashboard(text, version):
    normalized = re.sub(r"\s+", " ", text).lower()
    # Dashboard in the tab bar alone does not prove that the body rendered.
    for marker in ("universal productivity companion", "customize", "command center", "configure accounts"):
        if marker not in normalized:
            raise RuntimeError(f"Dashboard screenshot is missing {marker!r}")
    require_version(normalized, version)


def inspect_screenshot(path, version, menu=False):
    attempts, failures = [], []
    # Sparse text can split a version number at 110% scale. Try bounded page
    # layouts on the SAME untouched PNG, requiring every marker in ONE result.
    # No digit substitutions, substring matches, action replay, or merged OCR.
    for mode, scale in ((11, 1), (3, 1), (6, 1), (11, 3)):
        source = path
        if scale != 1:
            # Analysis only: retain the original window pixels for review.
            # Small version glyphs can acquire punctuation at native resolution.
            source = path.with_suffix(".ocr-analysis-3x.png")
            run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
                "-vf", "scale=iw*3:ih*3:flags=neighbor", "-frames:v", "1", source)
        text = run("tesseract", source, "stdout", "--psm", mode)
        attempts.append(dict(mode=mode, scale=scale, text=text))
        try:
            if menu:
                # Assert this state's content. Dashboard/card content is checked
                # separately before and after this overlay obscures its pixels.
                require_version(text, version)
                if not all(re.search(rf"\b{marker}\b", text, re.IGNORECASE) for marker in ("file", "exit")):
                    raise RuntimeError("Tab and Enter did not open the File menu")
            else:
                require_dashboard(text, version)
        except RuntimeError as error:
            failures.append(error)
            continue
        path.with_suffix(".ocr.txt").write_text(text, encoding="utf-8")
        path.with_suffix(".ocr-mode.json").write_text(json.dumps(dict(mode=mode, scale=scale)), encoding="utf-8")
        return
    primary = failures[0]
    finish_cleanup(lambda: path.with_suffix(".ocr.txt").write_text(
        "\n\n".join(f'PSM {item["mode"]}, scale {item["scale"]}\n{item["text"]}' for item in attempts), encoding="utf-8"),
        primary, "Rejected OCR evidence")
    finish_cleanup(lambda: print(f"Dashboard OCR rejected {path.name} at original scale and 3x analysis", file=sys.stderr),
                   primary, "OCR diagnostics")
    raise primary


def inspect_keyboard_menu(path, version):
    inspect_screenshot(path, version, menu=True)


def finish_cleanup(cleanup, primary, operation):
    """Required cleanup fails a successful run, but cannot hide its primary error."""
    try:
        cleanup()
    except Exception as secondary:
        if primary is None:
            raise
        notice = f"{operation} also failed ({type(secondary).__name__}); primary failure retained."
        if hasattr(primary, "add_note"):
            primary.add_note(notice)
        else:
            # Older Python hosts lack exception notes. Even a broken diagnostic
            # stream must not replace the already-observed operation failure.
            try:
                print(notice, file=sys.stderr)
            except Exception:
                pass


@contextmanager
def recording(window, directory):
    directory.mkdir()
    stop = Event()
    def capture():
        index = 0
        while not stop.is_set():
            window.screenshot(directory / f"frame-{index:04}.png")
            index += 1
            stop.wait(0.25)
        if index < 2:
            raise RuntimeError("Interaction recording captured fewer than two frames")
    with ThreadPoolExecutor(max_workers=1) as executor:
        future = executor.submit(capture)
        try:
            yield
        finally:
            primary = sys.exc_info()[1]
            stop.set()
            finish_cleanup(lambda: future.result(timeout=15), primary, "Interaction recording")


def encode_recording(directory):
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-framerate", "4",
        "-i", directory / "frames" / "frame-%04d.png",
        "-vf", "scale=1600:1000:force_original_aspect_ratio=decrease:force_divisible_by=2,pad=1600:1000:(ow-iw)/2:(oh-ih)/2",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
        directory / "interaction.mp4")


def fixture(directory, theme):
    directory.mkdir(parents=True)
    config = directory / "config.json"
    # SettingsAppearance persists the six preset colors when changing theme.
    # A theme name alone leaves UiConfig's dark defaults as explicit overrides.
    # Match src/components/settings/appearanceUtils.ts rather than manufacturing
    # a mixed light-titlebar/dark-content profile that the settings UI never emits.
    appearance = {
        "dark": dict(accentColor="#0e639c", fontColor="#cccccc", bgPrimary="#1e1e1e",
                     bgSecondary="#252526", statusBarBg="#181818", statusBarFg="#9d9d9d"),
        "light": dict(accentColor="#0078d4", fontColor="#1f1f1f", bgPrimary="#ffffff",
                      bgSecondary="#f3f3f3", statusBarBg="#f3f3f3", statusBarFg="#616161"),
    }[theme]
    config.write_text(json.dumps({
        "ui": {"theme": theme, **appearance, "dashboardCards": {
            "weather": False, "workspace-pulse": False,
            "command-center": True, "finance": False,
        }},
        "github": {"accounts": []}, "native": {"activeSection": None},
        "packageQualificationSentinel": {"preserve": True},
    }), encoding="utf-8")
    # Cross-app Electron geometry format, without synthetic native metadata.
    (directory / "window-state.json").write_text(json.dumps({
        "x": 80, "y": 80, "width": 1200, "height": 820,
        "isMaximized": False, "isFullScreen": False,
    }), encoding="utf-8")
    return config


def windows(executable, directory, config, restored=False):
    actions = ["geometry", f"capture:{directory / 'dashboard.png'}"]
    if not restored:
        actions += [
            "record-start:" + str(directory / "frames"),
            "focus", "key:9", "key:13",
            f"capture:{directory / 'keyboard-menu.png'}", "key:27",
            "resize:80:80:900:650", "geometry",
            f"capture:{directory / 'resized.png'}", "focus", "ctrlkey:187",
            f"capture:{directory / 'zoomed.png'}",
            # At 96 DPI the 800x600 client minimum has a 16x8 Windows frame.
            # Its 125% outer size fits a 1024x768 desktop's tracking limits.
            "resize:80:40:816:608",
            "simulated-dpi:120:1020:760",
            f"capture:{directory / 'dpi-simulated-125-percent.png'}",
            "simulated-dpi:96:816:608",
            "resize:80:80:900:650",
            f"classicon:{directory / 'registered-window-icons.png'}",
            "record-stop",
        ]
    actions += ["ctrlkey:81"]
    run("pwsh", "-NoProfile", "-File", HERE / "windows-window.ps1",
        "-Exe", executable, "-OutDir", directory, "-ConfigPath", config,
        # Allow the driver's 45s render, 120s action, and 20s exit deadlines,
        # plus PowerShell startup and its owned-process cleanup.
        "-Actions", ",".join(actions), timeout=240)


def require_size(actual, width, height):
    if (not all(math.isfinite(actual[key]) for key in ("width", "height"))
            or abs(actual["width"] - width) > 2 or abs(actual["height"] - height) > 2):
        raise RuntimeError(f"Resize was not applied: requested {width}x{height}, observed {actual}")


def require_dpi_sequence(directory):
    # Both the scale-up and the return event must actually resize the HWND.
    for dpi, width, height in ((120, 1020, 760), (96, 816, 608)):
        actual = json.loads((directory / f"dpi-{dpi}.json").read_text(encoding="utf-8-sig"))
        if actual["simulatedDpi"] != dpi or not math.isfinite(actual["actualMonitorDpi"]) or actual["actualMonitorDpi"] <= 0:
            raise RuntimeError("Unreadable simulated versus actual monitor DPI")
        require_size(actual, width, height)


def owned_wayland_node(tree, pid, window=None):
    if (tree.get("pid") == pid and tree.get("app_id") == "buddy-native"
            and (window is None or tree.get("id") == window)):
        return tree
    for child in tree.get("nodes", []) + tree.get("floating_nodes", []):
        found = owned_wayland_node(child, pid, window)
        if found is not None:
            return found
    return None


class UnixWindow:
    def __init__(self, process, session):
        self.process = process
        self.session = session
        self.window = None

    def ready(self):
        if self.session == "macos":
            # System Events scopes every action to the owned PID.
            result = run("osascript", "-e", self.apple("get count of windows"))
            return int(result) > 0
        if self.session == "x11":
            self.window = run("xdotool", "search", "--onlyvisible", "--pid", self.process.pid).splitlines()[0]
            return True
        tree = json.loads(run("swaymsg", "-t", "get_tree", "-r"))
        found = owned_wayland_node(tree, self.process.pid)
        self.window = found["id"] if found is not None else None
        return self.window is not None

    def wayland_rect(self):
        tree = json.loads(run("swaymsg", "-t", "get_tree", "-r"))
        owned = owned_wayland_node(tree, self.process.pid, self.window)
        if owned is None:
            raise RuntimeError("Owned Wayland window disappeared")
        rect = owned["rect"]
        if rect["width"] <= 0 or rect["height"] <= 0:
            raise RuntimeError("Owned Wayland window has empty bounds")
        return rect

    def apple(self, command):
        return (f'tell application "System Events" to tell first application process '
                f'whose unix id is {self.process.pid}\n{command}\nend tell')

    def resize(self, width, height):
        if self.session == "macos":
            run("osascript", "-e", self.apple(f"set size of window 1 to {{{width}, {height}}}"))
        elif self.session == "x11":
            run("xdotool", "windowsize", self.window, width, height)
        else:
            run("swaymsg", f"[con_id={self.window}] floating enable, resize set {width} {height}, move position 80 80")
        time.sleep(1)
        if self.session == "macos":
            size = [int(value) for value in re.findall(r"-?\d+", run("osascript", "-e", self.apple("get size of window 1")))]
            if len(size) != 2:
                raise RuntimeError("Unreadable owned Mac window size")
            actual = dict(width=size[0], height=size[1], units="macOS window points")
        elif self.session == "x11":
            properties = dict(line.split("=", 1) for line in run("xdotool", "getwindowgeometry", "--shell", self.window).splitlines())
            actual = dict(width=int(properties["WIDTH"]), height=int(properties["HEIGHT"]), units="X11 client pixels")
        else:
            actual = dict(self.wayland_rect(), units="Wayland compositor units")
        require_size(actual, width, height)
        return actual

    def key(self, key, control=True):
        if self.session == "macos":
            command = (f'keystroke "{key}" using command down' if control else
                       f'key code {dict(Tab=48, Return=36, Escape=53)[key]}')
            run("osascript", "-e", self.apple(f'set frontmost to true\n{command}'))
        elif self.session == "x11":
            if control:
                # X11 sends a key to the deepest window under the pointer, which
                # is a web page's own window when the pointer rests over one, so
                # an app shortcut then never reaches Buddy (docs/NATIVE-BROWSER.md).
                # Park the pointer on the title bar, as a user would have to.
                run("xdotool", "mousemove", "--window", self.window, 600, 15)
            run("xdotool", "windowactivate", "--sync", self.window)
            run("xdotool", "key", "--clearmodifiers", f"ctrl+{key}" if control else key)
        else:
            run("swaymsg", f"[con_id={self.window}] focus")
            run("wtype", *( ["-M", "ctrl", "-k", key, "-m", "ctrl"] if control else ["-k", key]))
        time.sleep(0.7)

    def keyboard_menu(self, path):
        if self.session != "wayland":
            self.key("Tab", control=False)
            self.key("Return", control=False)
            self.screenshot(path)
            self.key("Escape", control=False)
            return
        run("swaymsg", f"[con_id={self.window}] focus")
        # Destroying wtype's virtual keyboard can remove seat focus and dismiss
        # the popup. Keep ONE keyboard attached through navigation and capture.
        keyboard = subprocess.Popen(
            ["wtype", "-s", "200", "-k", "Tab", "-s", "700", "-k", "Return", "-s", "7000", "-k", "Escape"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            time.sleep(1.6)
            if keyboard.poll() is not None:
                raise RuntimeError("Wayland virtual keyboard ended before the menu capture")
            self.screenshot(path)
            if keyboard.wait(timeout=15) != 0:
                raise RuntimeError("Wayland virtual keyboard failed")
        finally:
            def stop_keyboard():
                if keyboard.poll() is None:
                    keyboard.kill()
                    keyboard.wait(timeout=10)
            finish_cleanup(stop_keyboard, sys.exc_info()[1], "Virtual keyboard shutdown")

    def screenshot(self, path):
        if self.session == "macos":
            # Capture the owned app window only. No full-monitor recording.
            window = json.loads(run(Path(os.environ["RUNNER_TEMP"]) / "buddy-window-info", self.process.pid))
            # Exclude the OS shadow so PNG pixels / window points measures the
            # backing scale, rather than accidentally counting shadow margins.
            run("screencapture", "-x", "-o", "-l", window["kCGWindowNumber"], path)
            pixels = struct.unpack(">II", path.read_bytes()[16:24])
            window["capturePixelSize"] = pixels
            window["capturePixelsPerWindowPoint"] = pixels[0] / window["kCGWindowBounds"]["Width"]
            path.with_suffix(".window.json").write_text(json.dumps(window), encoding="utf-8")
        elif self.session == "x11":
            run("import", "-window", self.window, path)
        else:
            # Ask the compositor to capture this owned window region directly,
            # like the window-only captures on the other platforms. No PNG edit.
            rect = self.wayland_rect()
            run("grim", "-g", f'{rect["x"]},{rect["y"]} {rect["width"]}x{rect["height"]}', path)
            path.with_suffix(".window.json").write_text(json.dumps(rect), encoding="utf-8")


@contextmanager
def wayland_keyboard(session, directory):
    if session != "wayland":
        yield
        return
    # A headless seat has no physical keyboard. Transient wtype clients can
    # otherwise remove its last keyboard between actions and lose seat focus.
    # This owned client sends no input during the bounded app run.
    with (directory / "virtual-keyboard.log").open("w", encoding="utf-8") as log:
        keyboard = subprocess.Popen(
            ["wtype", "-s", "300000", "-k", "F15"], stdout=log, stderr=log)
        try:
            time.sleep(0.3)
            if keyboard.poll() is not None:
                raise RuntimeError("Wayland seat keyboard did not remain attached")
            yield
        finally:
            def stop_keyboard():
                if keyboard.poll() is None:
                    keyboard.kill()
                keyboard.wait(timeout=10)
            finish_cleanup(stop_keyboard, sys.exc_info()[1], "Seat keyboard shutdown")


def unix(executable, directory, config, session, restored=False):
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Unix input automation requires an ephemeral GitHub Actions desktop")
    env = dict(os.environ, BUDDY_CONFIG_PATH=str(config),
               BUDDY_CONVEX_URL="http://127.0.0.1:9", GH_CONFIG_DIR=str(directory / "empty-gh"))
    env.pop("GH_TOKEN", None)
    env.pop("GITHUB_TOKEN", None)
    if session == "x11":
        env.pop("WAYLAND_DISPLAY", None)
    elif session == "wayland":
        env.pop("DISPLAY", None)  # A passing XWayland launch cannot qualify Wayland.
    with wayland_keyboard(session, directory), (directory / "app.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen([str(executable)], cwd=directory, env=env, stdout=log, stderr=log)
        try:
            window = UnixWindow(process, session)
            deadline = time.monotonic() + 45
            last_error = None
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"App exited before rendering: {process.returncode}")
                try:
                    if window.ready():
                        break
                except (subprocess.SubprocessError, RuntimeError, IndexError, ValueError) as error:
                    last_error = error
                time.sleep(0.3)
            else:
                raise RuntimeError(f"Owned Buddy window did not appear: {last_error}") from last_error
            time.sleep(4)
            window.screenshot(directory / "dashboard.png")
            if not restored:
                with recording(window, directory / "frames"):
                    window.keyboard_menu(directory / "keyboard-menu.png")
                    actual = window.resize(900, 650)
                    (directory / "resize.json").write_text(json.dumps(actual), encoding="utf-8")
                    window.screenshot(directory / "resized.png")
                    window.key("=" if session == "macos" else "equal")
                    window.screenshot(directory / "zoomed.png")
                    time.sleep(1)
            window.key("q")
            if process.wait(timeout=20) != 0:
                raise RuntimeError(f"App did not exit cleanly: {process.returncode}")
        finally:
            def stop_child():
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)
            finish_cleanup(stop_child, sys.exc_info()[1], "App shutdown")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11", "wayland"), required=True)
    args = parser.parse_args()
    executable = args.executable.resolve(strict=True)
    output = args.output.resolve()
    if output.exists():
        raise RuntimeError("Smoke evidence requires a fresh output directory")
    output.mkdir(parents=True)
    observed = run(executable, "--version", cwd=output)
    if observed != f"Buddy Native {args.version}":
        raise RuntimeError(f"Installed version mismatch: {observed}")
    if args.session in ("x11", "wayland"):
        libraries = run("ldd", executable)
        (output / "runtime-libraries.txt").write_text(libraries, encoding="utf-8")
        if "not found" in libraries:
            raise RuntimeError("Installed executable has missing runtime libraries")
    drive = windows if args.session == "windows" else (
        lambda exe, directory, config, restored=False: unix(exe, directory, config, args.session, restored))
    for theme in ("dark", "light"):
        directory = output / theme
        config = fixture(directory, theme)
        drive(executable, directory, config)
        require_size(json.loads((directory / "resize.json").read_text(encoding="utf-8-sig")), 900, 650)
        if args.session == "windows":
            require_dpi_sequence(directory)
        for shot in ("dashboard", "resized", "zoomed"):
            inspect_screenshot(directory / f"{shot}.png", args.version)
        inspect_keyboard_menu(directory / "keyboard-menu.png", args.version)
        encode_recording(directory)
        if args.session == "windows":
            inspect_screenshot(directory / "dpi-simulated-125-percent.png", args.version)
        zoom = json.loads((directory / "zoom-level.json").read_text(encoding="utf-8-sig"))
        if abs(zoom["zoomFactor"] - 1.1) > 0.001:
            raise RuntimeError("Keyboard zoom did not persist 110%")
        saved = json.loads((directory / "window-state.json").read_text(encoding="utf-8-sig"))
        (directory / "before-restart.json").write_text(json.dumps(saved), encoding="utf-8")
        restored = directory / "restored"
        restored.mkdir()
        drive(executable, restored, config, restored=True)
        inspect_screenshot(restored / "dashboard.png", args.version)
        if args.session == "wayland":
            require_size(json.loads((restored / "dashboard.window.json").read_text()), 900, 650)
        after = json.loads((directory / "window-state.json").read_text(encoding="utf-8-sig"))
        # Wayland's compositor chooses global placement; the app restores size.
        geometry_keys = ("width", "height") if args.session == "wayland" else ("x", "y", "width", "height")
        for key in geometry_keys:
            if abs(saved[key] - after[key]) > 2:
                raise RuntimeError(f"Saved geometry changed after restart: {key}")
        # A deliberately disconnected monitor must recover to a visible window.
        after.update(x=50000, y=50000, nativeDisplayUuid="qualification-disconnected-display",
                     displayBounds={"x": 50000, "y": 50000, "width": 1920, "height": 1080})
        after.pop("nativeGeometry", None)
        (directory / "window-state.json").write_text(json.dumps(after), encoding="utf-8")
        recovery = directory / "disconnected-display"
        recovery.mkdir()
        drive(executable, recovery, config, restored=True)
        inspect_screenshot(recovery / "dashboard.png", args.version)
        if args.session == "wayland":
            require_size(json.loads((recovery / "dashboard.window.json").read_text()), 900, 650)
        recovered = json.loads((directory / "window-state.json").read_text(encoding="utf-8-sig"))
        if recovered["x"] >= 50000 or recovered["y"] >= 50000:
            raise RuntimeError("Disconnected-display recovery did not persist a usable placement")
        if json.loads(config.read_text(encoding="utf-8-sig"))["packageQualificationSentinel"] != {"preserve": True}:
            raise RuntimeError("Unknown cross-app settings were lost")
    (output / "qualification.json").write_text(json.dumps({
        "version": args.version, "session": args.session, "platform": platform.platform(),
        "architecture": platform.machine(), "binarySha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
        "result": "passed",
    }, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
