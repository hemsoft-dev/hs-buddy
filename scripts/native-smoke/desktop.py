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
    try:
        return subprocess.run(
            [str(arg) for arg in args], check=True, capture_output=True,
            text=True, timeout=60, **kwargs,
        ).stdout.strip()
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"{args[0]} failed with exit {error.returncode}: {error.stderr[-8000:]}") from error


def require_dashboard(text, version):
    normalized = re.sub(r"\s+", " ", text).lower()
    # Dashboard in the tab bar alone does not prove that the body rendered.
    for marker in ("universal productivity companion", "customize", version):
        if marker not in normalized:
            raise RuntimeError(f"Dashboard screenshot is missing {marker!r}")


def inspect_screenshot(path, version):
    text = run("tesseract", path, "stdout", "--psm", "11")
    path.with_suffix(".ocr.txt").write_text(text, encoding="utf-8")
    require_dashboard(text, version)


def inspect_keyboard_menu(path, version):
    inspect_screenshot(path, version)
    text = path.with_suffix(".ocr.txt").read_text(encoding="utf-8")
    if not re.search(r"\bexit\b", text, re.IGNORECASE):
        raise RuntimeError("Tab and Enter did not open the File menu")


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
            "command-center": False, "finance": False,
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
            "simulated-dpi:144:1350:975",
            f"capture:{directory / 'dpi-simulated-150-percent.png'}",
            "simulated-dpi:96:900:650",
            f"classicon:{directory / 'registered-window-icons.png'}",
            "record-stop",
        ]
    actions += ["ctrlkey:81"]
    run("pwsh", "-NoProfile", "-File", HERE / "windows-window.ps1",
        "-Exe", executable, "-OutDir", directory, "-ConfigPath", config,
        "-Actions", ",".join(actions))


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
        def find(node):
            if node.get("pid") == self.process.pid and node.get("app_id") == "buddy-native":
                return node["id"]
            for child in node.get("nodes", []) + node.get("floating_nodes", []):
                match = find(child)
                if match:
                    return match
            return None
        self.window = find(tree)
        return self.window is not None

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

    def key(self, key, control=True):
        if self.session == "macos":
            command = (f'keystroke "{key}" using command down' if control else
                       f'key code {dict(Tab=48, Return=36, Escape=53)[key]}')
            run("osascript", "-e", self.apple(f'set frontmost to true\n{command}'))
        elif self.session == "x11":
            run("xdotool", "windowactivate", "--sync", self.window)
            run("xdotool", "key", "--clearmodifiers", f"ctrl+{key}" if control else key)
        else:
            run("swaymsg", f"[con_id={self.window}] focus")
            run("wtype", *( ["-M", "ctrl", "-k", key, "-m", "ctrl"] if control else ["-k", key]))
        time.sleep(0.7)

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
            # Sway runs on a fresh headless output that contains only this app.
            run("grim", path)


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
    with (directory / "app.log").open("w", encoding="utf-8") as log:
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
                    window.key("Tab", control=False)
                    window.key("Return", control=False)
                    window.screenshot(directory / "keyboard-menu.png")
                    window.key("Escape", control=False)
                    window.resize(900, 650)
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
        for shot in ("dashboard", "resized", "zoomed"):
            inspect_screenshot(directory / f"{shot}.png", args.version)
        inspect_keyboard_menu(directory / "keyboard-menu.png", args.version)
        encode_recording(directory)
        if args.session == "windows":
            inspect_screenshot(directory / "dpi-simulated-150-percent.png", args.version)
        zoom = json.loads((directory / "zoom-level.json").read_text(encoding="utf-8-sig"))
        if abs(zoom["zoomFactor"] - 1.1) > 0.001:
            raise RuntimeError("Keyboard zoom did not persist 110%")
        saved = json.loads((directory / "window-state.json").read_text(encoding="utf-8-sig"))
        (directory / "before-restart.json").write_text(json.dumps(saved), encoding="utf-8")
        restored = directory / "restored"
        restored.mkdir()
        drive(executable, restored, config, restored=True)
        inspect_screenshot(restored / "dashboard.png", args.version)
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
