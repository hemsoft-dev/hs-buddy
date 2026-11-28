"""Exercise an installed Buddy package, using only an isolated CI desktop.

Windows uses a never-shown desktop and posted messages. Unix desktop drivers
require GitHub Actions so they cannot send input to a developer's desktop.
Screenshots contain real OS window pixels; OCR must find dashboard content.
"""

import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager, nullcontext
import hashlib
import importlib.util
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
from threading import Event, RLock
from receipt_clock import stamp


HERE = Path(__file__).resolve().parent
# Alt (Option) chords the drivers send: macOS key codes, X11 keysyms, Windows VKs.
ALT_KEYS = {"=": dict(mac=24, x11="equal", windows=0xBB), "-": dict(mac=27, x11="minus", windows=0xBD)}


def run(*args, **kwargs):
    timeout = kwargs.pop("timeout", 60)
    # Pipes redirect output but do not suppress Windows console creation.
    # This includes the outer PowerShell driver for a never-shown desktop.
    if os.name == "nt":
        kwargs["creationflags"] = kwargs.get("creationflags", 0) | subprocess.CREATE_NO_WINDOW
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


def require_closed_sessions(before, after, launches):
    """Each installed launch is one Buddy session, as in Electron's
    useAppSessionStats: the start counts appLaunches, the quit ends the session
    and adds its uptime, so none is left open. Uptime depends on the clock,
    so only its growth is required. Without a launch nothing may change."""
    launched = after.get("appLaunches", 0) - before.get("appLaunches", 0)
    uptime = after.get("totalUptimeMs", 0) - before.get("totalUptimeMs", 0)
    session = before.get("lastSessionStart") if launches == 0 else None
    if (launched != launches or uptime < 0 or (uptime > 0) != (launches > 0)
            or after.get("lastSessionStart") != session):
        raise RuntimeError(f"Native sessions differ: {launched} launches, {uptime} ms uptime, "
            f"open={after.get('lastSessionStart') is not None}; expected {launches} closed sessions")


def require_dashboard(text, version):
    normalized = re.sub(r"\s+", " ", text).lower()
    # Dashboard in the tab bar alone does not prove that the body rendered.
    for marker in ("universal productivity companion", "customize", "command center", "configure accounts"):
        if marker not in normalized:
            raise RuntimeError(f"Dashboard screenshot is missing {marker!r}")
    require_version(normalized, version)


# One fixture quote that a previous run's fetch would have cached. The symbol
# is not listed anywhere, so even a fetch that escaped the closed proxy fails.
FINANCE_FIXTURE = dict(symbol="HSBUDDY.FIXTURE", name="Qualification Quote", price=987.65, change_percent=0.37)
# Every HTTP(S) request of the relaunch goes to a closed loopback port.
OFFLINE_PROXY = "http://127.0.0.1:9"
PROXY_VARIABLES = ("HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy")
# What the collapsed Finance card shows once a refresh failed with quotes on
# screen (stale_notice in cards/finance.rs): the sign that the refresh
# finished. The expanded card shows only the provider's error, which differs
# with how the request failed.
STALE_NOTICE = "Showing the last successful quotes"


def seed_finance_cache(config, cache_dir, now_ms=None):
    """Show only the Finance card, saved as collapsed, for the fixture symbol,
    and leave a fresh native quote cache in `cache_dir` (finance-quotes.json,
    Electron's finance:cache document plus its watchlist), as a successful
    fetch in the previous run would. Returns the cache path."""
    settings = json.loads(config.read_text(encoding="utf-8-sig"))
    settings["ui"]["dashboardCards"].update({"finance": True, "command-center": False})
    settings.setdefault("native", {}).setdefault("expandedCards", {})["finance"] = False
    settings.setdefault("finance", {})["watchlist"] = [FINANCE_FIXTURE["symbol"]]
    config.write_text(json.dumps(settings), encoding="utf-8")
    price = FINANCE_FIXTURE["price"]
    quote = dict(symbol=FINANCE_FIXTURE["symbol"], name=FINANCE_FIXTURE["name"], price=price,
                 change=round(price * FINANCE_FIXTURE["change_percent"] / 100, 2),
                 changePercent=FINANCE_FIXTURE["change_percent"],
                 previousClose=round(price - 3.64, 2), marketOpen=True)
    cache_dir.mkdir(parents=True, exist_ok=True)
    cache = cache_dir / "finance-quotes.json"
    cache.write_text(json.dumps(dict(
        quotes=[quote], timestamp=int(time.time() * 1000) if now_ms is None else now_ms,
        version=3, symbols=[FINANCE_FIXTURE["symbol"]])), encoding="utf-8")
    return cache


@contextmanager
def isolated_cache(cache_dir):
    """Point the launched app's cache directory (BUDDY_CACHE_DIR) at `cache_dir`."""
    saved = os.environ.get("BUDDY_CACHE_DIR")
    os.environ["BUDDY_CACHE_DIR"] = str(cache_dir)
    try:
        yield
    finally:
        if saved is None:
            os.environ.pop("BUDDY_CACHE_DIR", None)
        else:
            os.environ["BUDDY_CACHE_DIR"] = saved


@contextmanager
def offline_network():
    """Route the launched app's HTTP(S) through a closed loopback port, so its
    quote refresh fails and only the cache can fill the Finance card."""
    names = PROXY_VARIABLES + ("NO_PROXY", "no_proxy")
    saved = {name: os.environ.get(name) for name in names}
    for name in names:
        os.environ.pop(name, None)
    os.environ.update({name: OFFLINE_PROXY for name in PROXY_VARIABLES})
    try:
        yield
    finally:
        for name, value in saved.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def require_cached_finance(text, version):
    """The collapsed Finance card shows the cached fixture quote, its name and
    change, with the notice that the refresh failed and the cache is shown."""
    normalized = re.sub(r"\s+", " ", text).lower()
    for marker in ("finance", "qualification", "last successful quotes"):
        if marker not in normalized:
            raise RuntimeError(f"Finance screenshot is missing {marker!r}")
    if not re.search(r"(?<![\d.,])0[.,]37\s*%", text):
        raise RuntimeError("Finance screenshot is missing the cached change 0.37%")
    require_version(normalized, version)


def inspect_screenshot(path, version, menu=False, require=None):
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
                (require or require_dashboard)(text, version)
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
        owned_mac = isinstance(window, UnixWindow) and window.session == "macos"
        was_recording = window._recording if owned_mac else False
        if owned_mac:
            window._recording = True
        try:
            future = executor.submit(capture)
            try:
                yield
            finally:
                primary = sys.exc_info()[1]
                stop.set()
                finish_cleanup(lambda: future.result(timeout=15), primary, "Interaction recording")
        finally:
            if owned_mac:
                window._recording = was_recording


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


def load_ocr():
    """settings-ocr.py, the locator the settings scene's waits use."""
    spec = importlib.util.spec_from_file_location("settings_ocr", HERE / "settings-ocr.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def wait_for_text(window, directory, text, timeout=30, locate=None):
    """Read the owned window until OCR finds `text` (settings-desktop's
    wait_visible). It gives up only after a frame captured at or after the
    deadline, so the whole wait is observed."""
    locate = locate or load_ocr().locate_image
    image = directory / "wait-status.png"
    deadline = time.monotonic() + timeout
    while True:
        started = time.monotonic()
        window.screenshot(image)
        try:
            locate(image, text)
            return
        except RuntimeError as failure:
            if started >= deadline:
                raise RuntimeError(f"The owned window did not show {text!r}") from failure
        time.sleep(0.2)


def windows(executable, directory, config, restored=False, ready_text=None):
    # A restored launch may wait for a status before its capture.
    wait = [f"waittext64:{base64.b64encode(ready_text.encode()).decode()}"] if ready_text else []
    actions = ["geometry", *wait, f"capture:{directory / 'dashboard.png'}"]
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
        self._recording = False
        self._recording_input_lock = RLock()

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

    def hover(self, where="page"):
        """Rests the pointer in the middle of the window, over the page when a
        browser tab shows one, or on the title bar's empty middle. Only a move
        over Buddy's own window reaches GPUI, so only "title" clears a tooltip
        left from where the pointer was."""
        if self.session != "x11":
            raise RuntimeError("Only the X11 driver rests the pointer over a page")
        properties = dict(line.split("=", 1) for line in run("xdotool", "getwindowgeometry", "--shell", self.window).splitlines())
        x, y = int(properties["WIDTH"]) // 2, int(properties["HEIGHT"]) // 2
        if where == "title":
            y = 15
        elif where != "page":
            raise RuntimeError(f"Unknown pointer rest: {where}")
        run("xdotool", "mousemove", "--window", self.window, x, y)
        time.sleep(0.3)

    def resize(self, width, height, *, initial_origin=None):
        if initial_origin is not None and self.session != "macos":
            raise RuntimeError("Initial placement is Mac comparison only")
        if self.session == "macos":
            command = f"set size of window 1 to {{{width}, {height}}}"
            if initial_origin is not None:
                placement = getattr(self, "_placement", None)
                if placement is None:
                    raise RuntimeError("Initial placement requires owned comparison admission")
                command += "\n" + placement.geometry(self, initial_origin, (width, height))
            run("osascript", "-e", self.apple(command))
        elif self.session == "x11":
            run("xdotool", "windowsize", self.window, width, height)
        else:
            run("swaymsg", f"[con_id={self.window}] floating enable, resize set {width} {height}, move position 80 80")
        time.sleep(1)
        if self.session == "macos":
            query = "get size of window 1" if initial_origin is None else "return {size of window 1, position of window 1}"
            size = [int(value) for value in re.findall(r"-?\d+", run("osascript", "-e", self.apple(query)))]
            if initial_origin is not None:
                placement.assigned_geometry(size)
                size = size[:2]
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
            # The pointer stays where it is: over a web page, X11 sends the key
            # to the page's window, which hands Buddy's shortcuts back (#880).
            run("xdotool", "windowactivate", "--sync", self.window)
            run("xdotool", "key", "--clearmodifiers", f"ctrl+{key}" if control else key)
        else:
            run("swaymsg", f"[con_id={self.window}] focus")
            run("wtype", *( ["-M", "ctrl", "-k", key, "-m", "ctrl"] if control else ["-k", key]))
        time.sleep(0.7)

    def alt_key(self, key):
        """Alt (Option on macOS) with `=` or `-`, delivered to Buddy itself. On
        X11 the pointer first rests on the title bar: a key goes to the window
        under the pointer, and a page keeps Alt keys, as an Electron webview does."""
        if key not in ALT_KEYS:
            raise RuntimeError(f"Unsupported Alt key: {key}")
        if self.session == "macos":
            run("osascript", "-e", self.apple(f"set frontmost to true\nkey code {ALT_KEYS[key]['mac']} using option down"))
        elif self.session == "x11":
            self.hover("title")
            run("xdotool", "windowactivate", "--sync", self.window)
            run("xdotool", "key", "--clearmodifiers", f"alt+{ALT_KEYS[key]['x11']}")
        else:
            run("swaymsg", f"[con_id={self.window}] focus")
            run("wtype", "-M", "alt", "-k", ALT_KEYS[key]["x11"], "-m", "alt")
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
        with mac_input_guard(self):
            self._screenshot(path)

    def _screenshot(self, path):
        if self.session == "macos":
            started = stamp()
            # Capture the owned app window only. No full-monitor recording.
            placement = getattr(self, "_placement", None)
            options = {"env": placement.capture_environment()} if placement is not None else {}
            window = json.loads(run(Path(os.environ["RUNNER_TEMP"]) / "buddy-window-info", self.process.pid, **options))
            # Exclude the OS shadow so PNG pixels / window points measures the
            # backing scale, rather than accidentally counting shadow margins.
            run("screencapture", "-x", "-o", "-l", window["kCGWindowNumber"], path)
            pixels = struct.unpack(">II", path.read_bytes()[16:24])
            window["captureTiming"] = dict(start=started, end=stamp())
            window["capturePixelSize"] = pixels
            window["capturePixelsPerWindowPoint"] = pixels[0] / window["kCGWindowBounds"]["Width"]
            path.with_suffix(".window.json").write_text(json.dumps(window), encoding="utf-8")
            if placement is not None:
                placement.capture(window)
        elif self.session == "x11":
            # Read visible screen pixels within the owned window bounds. A
            # direct XGetImage of the client can omit overlapping surfaces.
            run("import", "-screen", "-window", self.window, path)
        else:
            # Ask the compositor to capture this owned window region directly,
            # like the window-only captures on the other platforms. No PNG edit.
            rect = self.wayland_rect()
            run("grim", "-g", f'{rect["x"]},{rect["y"]} {rect["width"]}x{rect["height"]}', path)
            path.with_suffix(".window.json").write_text(json.dumps(rect), encoding="utf-8")


def mac_input_guard(window):
    """Only a recorded owned Mac window excludes its camera during a click."""
    if isinstance(window, UnixWindow) and window.session == "macos" and window._recording:
        return window._recording_input_lock
    return nullcontext()


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


def unix(executable, directory, config, session, restored=False, ready_text=None):
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
            if ready_text:
                wait_for_text(window, directory, ready_text)
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


def cached_finance(executable, directory, config, drive, version):
    """Relaunch offline with a fresh seeded quote cache in an isolated cache
    directory. The capture waits until the card says the refresh failed and
    it still shows the cached quote, so the refresh has finished; the cache
    must be as it was."""
    cache = seed_finance_cache(config, directory / "cache")
    seeded = cache.read_bytes()
    finance = directory / "finance-cached"
    finance.mkdir()
    with offline_network(), isolated_cache(cache.parent):
        drive(executable, finance, config, restored=True, ready_text=STALE_NOTICE)
    capture = finance / "finance-cached-dark.png"
    (finance / "dashboard.png").replace(capture)
    inspect_screenshot(capture, version, require=require_cached_finance)
    if cache.read_bytes() != seeded:
        raise RuntimeError("A failed quote refresh rewrote the cached quotes")


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
        lambda exe, directory, config, restored=False, ready_text=None: unix(
            exe, directory, config, args.session, restored, ready_text))
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
        if theme == "dark":
            cached_finance(executable, directory, config, drive, args.version)
    (output / "qualification.json").write_text(json.dumps({
        "version": args.version, "session": args.session, "platform": platform.platform(),
        "architecture": platform.machine(), "binarySha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
        "result": "passed",
    }, indent=2), encoding="utf-8")


if __name__ == "__main__":
    main()
