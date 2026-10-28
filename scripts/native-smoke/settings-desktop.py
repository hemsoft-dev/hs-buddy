"""Qualify real installed settings against a disposable anonymous backend."""
import argparse
import base64
from contextlib import contextmanager, nullcontext
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import uuid

from desktop import UnixWindow, encode_recording, finish_cleanup, fixture, recording, run


HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ocr = load("settings-ocr")
SettingsBackend = load("settings-backend").SettingsBackend
OwnedMacInput = load("macos-input").OwnedMacInput


@contextmanager
def unicode_clipboard(text):
    """Serve an exact UTF-8 selection only on an owned disposable X11 desktop."""
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1" or sys.platform != "linux":
        raise RuntimeError("Unicode clipboard input requires a disposable Linux runner")
    if shutil.which("xclip") is None:
        raise RuntimeError("The Linux desktop fixture requires xclip; install its shared desktop test tools first")
    payload = text.encode("utf-8")
    if len(payload) > 32768:
        raise RuntimeError("Fixture clipboard text exceeds its bounded input size")
    # -quiet keeps the selection owner in the foreground, so it can be reaped.
    # https://github.com/astrand/xclip
    owner = subprocess.Popen(["xclip", "-selection", "clipboard", "-in", "-quiet"],
        stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        owner.stdin.write(payload)
        owner.stdin.close()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            if owner.poll() is not None:
                raise RuntimeError("The owned clipboard process exited before paste")
            try:
                current = subprocess.run(["xclip", "-selection", "clipboard", "-out"],
                    capture_output=True, timeout=1)
            except subprocess.TimeoutExpired:
                continue
            if current.returncode == 0 and current.stdout == payload:
                break
            time.sleep(0.05)
        else:
            raise RuntimeError("The owned clipboard did not offer the exact Unicode text")
        yield
    finally:
        primary = sys.exc_info()[1]
        def stop():
            try:
                if owner.poll() is None:
                    owner.terminate()
                try:
                    owner.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    owner.kill()
                    owner.wait(timeout=3)
            finally:
                owner.stdin.close()
                owner.stderr.close()
        finish_cleanup(stop, primary, "Unicode clipboard owner")


# Vision's boxes for an unchanged Mac window can differ by a pixel or two between captures;
# four captures that never matched exactly failed qualification on unrelated stages (#868).
STABLE_SLACK = 3


def stable(current, previous, slack=STABLE_SLACK):
    """Whether two OCR measurements name the same on-screen target.

    Numbers may differ by `slack` pixels; everything else (keys, nesting, text) must match.
    """
    if previous is None or current is None:
        return False
    if isinstance(current, dict) and isinstance(previous, dict):
        return current.keys() == previous.keys() and all(
            stable(current[key], previous[key], slack) for key in current)
    if isinstance(current, (int, float)) and isinstance(previous, (int, float)) \
            and not isinstance(current, bool) and not isinstance(previous, bool):
        return abs(current - previous) <= slack
    return current == previous


def record_measurement(directory, name, value):
    """Keeps every Mac stability measurement, so a failure shows what moved."""
    if not isinstance(directory, Path) or not directory.is_dir():
        return
    with (directory / "stability.jsonl").open("a", encoding="utf-8") as log:
        log.write(json.dumps(dict(target=name, measurement=value)) + "\n")


def b64(value):
    return base64.b64encode(str(value).encode()).decode()


# Cold fixture discovery can use 95s preparation + 45s startup + 30s catalog
# request + 15s shutdown. Keep a bounded margin for filesystem cleanup/OCR.
MODEL_CATALOG_TIMEOUT = 200


def wait_visible(window, directory, text, timeout=15, scroll=None):
    """Read an owned window until one OCR layout has the exact visible status.

    By default this never posts input. A caller may explicitly scroll its
    owned content to reveal an offscreen status, while still requiring OCR.
    """
    image = directory / "wait-status.png"
    deadline = time.monotonic() + timeout
    while True:
        # A slow OCR pass can outlast the timeout. Give up only after a frame
        # captured at or after the deadline, so the full wait is observed.
        started = time.monotonic()
        window.screenshot(image)
        try:
            ocr.locate_image(image, text)
            return
        except RuntimeError as failure:
            if started >= deadline:
                raise RuntimeError(f"The owned window did not show the expected status {text!r}") from failure
            if scroll is not None:
                scroll()
        time.sleep(0.1)


def planned_large_size(width, height):
    size = (min(1200, int(width)), min(820, int(height)))
    if size[0] < 900 or size[1] < 650:
        raise RuntimeError("The desktop cannot fit the supported minimum window")
    return size


def prepare_companion(profile):
    if (profile / ".buddy-settings-fixture").read_text() != "buddy settings fixture v1":
        raise RuntimeError("Owned settings fixture marker required")
    for name in ("request", "ready", "stop", "complete", "failed"):
        (profile / f"companion-{name}.json").unlink(missing_ok=True)
    request = dict(runId=str(uuid.uuid4()))
    (profile / "companion-request.json").write_text(json.dumps(request), encoding="utf-8")
    return request


def current_companion(profile, name):
    request = json.loads((profile / "companion-request.json").read_text())
    receipt = json.loads((profile / f"companion-{name}.json").read_text())
    if receipt.get("runId") != request.get("runId"):
        raise RuntimeError("Companion receipt belongs to another run")
    return receipt


def stop_companion(profile):
    ready = current_companion(profile, "ready")
    target = profile / "companion-stop.json"
    temporary = target.with_suffix(f".{ready['runId']}.tmp")
    temporary.write_text(json.dumps(dict(runId=ready["runId"])), encoding="utf-8")
    temporary.replace(target)


def verify_companion_overlap(ready, receipt, captured):
    observed = receipt.get("observedNativeSaveAt")
    if (ready.get("runId") != receipt.get("runId") or observed is None
            or not ready["startedAt"] <= observed <= receipt["completedAt"]
            or not ready["startedAt"] <= captured <= receipt["completedAt"]
            or receipt.get("writesBeforeNativeSave", 0) < 1
            or receipt.get("writesAfterNativeSave", 0) < 1):
        raise RuntimeError("Native save was not observed during actual Electron writes")


def windows_large_size():
    import ctypes as c
    user = c.windll.user32
    user.SetThreadDpiAwarenessContext.argtypes = [c.c_void_p]
    user.SetThreadDpiAwarenessContext.restype = c.c_void_p
    previous = user.SetThreadDpiAwarenessContext(c.c_void_p(-4))
    if not previous:
        raise RuntimeError("Could not measure the Windows tracking limits at physical DPI")
    try:
        return planned_large_size(user.GetSystemMetrics(59), user.GetSystemMetrics(60))
    finally:
        user.SetThreadDpiAwarenessContext(previous)


class Driver:
    def __init__(self, executable, profile, session, backend, checkout):
        self.executable, self.profile, self.session = executable, profile, session
        self.backend, self.checkout = backend, checkout
        self.config = profile / "config.json"
        self.large_size = (1200, 820)
        if session == "windows":
            self.large_size = windows_large_size()
        if session == "macos":
            display = json.loads(Path(os.environ["BUDDY_SETTINGS_DISPLAY_RECEIPT"]).read_text())
            self.large_size = planned_large_size(display["largeWindow"]["width"], display["largeWindow"]["height"])
        self.electron = run("bun", "-e", "console.log(require('electron'))", cwd=checkout).splitlines()[-1]

    def drive(self, name, actions, concurrent=False, record=False):
        directory = self.profile / name
        directory.mkdir()
        if concurrent:
            prepare_companion(self.profile)
        if self.session == "windows":
            commands = [f"resize:80:40:{self.large_size[0]}:{self.large_size[1]}"]
            if record:
                commands += ["record-start:" + str(directory / "frames")]
            for action in actions:
                kind, *values = action
                if kind in ("nav", "tab", "click", "input", "field", "box", "leftfield", "rightfield", "save", "cell"):
                    # An adjacent Unix row control is not part of the Windows
                    # command. Explicit input spacing is shared by both drivers.
                    encoded = values[:2] if kind == "input" else values
                    # Windows aims every field at the window middle; a box is one.
                    command = ("field" if kind == "box" else kind) + "64:" + ":".join(map(b64, encoded))
                    if kind == "input" and len(values) > 2 and isinstance(values[2], dict):
                        hint = values[2]
                        if set(hint) != {"labelBottomOffset"} or type(hint["labelBottomOffset"]) is not int or not 20 <= hint["labelBottomOffset"] <= 40:
                            raise RuntimeError("The input needs a bounded label-to-editor offset")
                        command += ":" + str(hint["labelBottomOffset"])
                    commands.append(command)
                elif kind == "capture":
                    commands.append("capture:" + str(directory / (values[0] + ".png")))
                elif kind == "resize":
                    size = self.large_size if values == [1200, 820] else values
                    commands.append(f"resize:80:40:{size[0]}:{size[1]}")
                elif kind == "key":
                    commands.append("key:" + str(dict(Tab=9, Return=13, Escape=27)[values[0]]))
                elif kind in ("wait_text", "wait_scroll_text", "wait_scroll_above", "wait_scroll_up"):
                    encoded = values[:2] if kind in ("wait_scroll_text", "wait_scroll_up") else values[:1]
                    command = "waittext64:" if kind == "wait_text" else "waituptext64:" if kind == "wait_scroll_up" else "waitscrolltext64:"
                    commands.append(command + ":".join(map(b64, encoded)))
                elif kind == "wait_models":
                    commands.append("waitmodeltext64:" + b64(values[0]))
                elif kind == "reveal_text":
                    commands.append("revealtext64:" + ":".join(map(b64, values)))
                elif kind == "sleep":
                    commands.append("sleep:" + str(int(values[0] * 1000)))
                elif kind == "wheel":
                    commands.append("wheel:" + str(values[0]))
                else:
                    raise RuntimeError("Unknown settings action")
            if record:
                commands.append("record-stop")
            commands.append("ctrlkey:81")
            arguments = ["pwsh", "-NoProfile", "-File", HERE / "windows-window.ps1", "-Exe", self.executable,
                "-OutDir", directory, "-ConfigPath", self.config, "-ConvexUrl", self.backend.url, "-Actions", ",".join(commands)]
            if concurrent:
                arguments += ["-CompanionExe", self.electron, "-CompanionScript", self.profile]
            # Preserve the ordinary action budget around the separate cold-model
            # wait and around explicit sleeps for paced service work.
            extra = sum(MODEL_CATALOG_TIMEOUT - 15 for action in actions if action[0] == "wait_models")
            extra += math.ceil(sum(action[1] for action in actions if action[0] == "sleep"))
            run(*arguments, cwd=self.checkout, timeout=180 + extra)
        else:
            self.unix(directory, actions, concurrent, record)
        if record:
            encode_recording(directory)
        return directory

    def unix(self, directory, actions, concurrent, record):
        if os.environ.get("GITHUB_ACTIONS") != "true":
            raise RuntimeError("Unix settings input requires a disposable CI desktop")
        environment = dict(os.environ, RUST_LOG=(os.environ.get("RUST_LOG") or "warn") + ",buddy::settings_view=debug", BUDDY_CONFIG_PATH=str(self.config), BUDDY_CONVEX_URL=self.backend.url,
            GH_CONFIG_DIR=str(self.profile / "empty-gh"))
        for key in ("GH_TOKEN", "GITHUB_TOKEN", "ELECTRON_RUN_AS_NODE"):
            environment.pop(key, None)
        if self.session == "x11":
            environment.pop("WAYLAND_DISPLAY", None)
        companion = None
        with (directory / "app.log").open("w", encoding="utf-8") as log:
            process = subprocess.Popen([str(self.executable)], cwd=directory, env=environment, stdout=log, stderr=log)
            try:
                window = UnixWindow(process, self.session)
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline:
                    if process.poll() is not None:
                        raise RuntimeError("Installed app exited before settings rendered")
                    try:
                        if window.ready():
                            break
                    except (RuntimeError, IndexError, ValueError, subprocess.SubprocessError):
                        pass
                    time.sleep(0.25)
                else:
                    raise RuntimeError("Installed settings window did not appear")
                time.sleep(4)
                window.resize(*self.large_size)
                if concurrent:
                    companion = subprocess.Popen([self.electron, str(self.profile)], cwd=self.profile,
                        env=environment, stdout=log, stderr=log)
                    deadline = time.monotonic() + 20
                    while not (self.profile / "companion-ready.json").exists():
                        if companion.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("Electron config companion did not become ready")
                        time.sleep(0.1)
                    current_companion(self.profile, "ready")
                with recording(window, directory / "frames") if record else nullcontext():
                    for index, (kind, *values) in enumerate(actions):
                        with (directory / "actions.jsonl").open("a", encoding="utf-8") as receipt:
                            receipt.write(json.dumps(dict(index=index, kind=kind,
                                label=values[0] if kind in ("nav", "tab", "click", "input", "field", "box", "leftfield", "rightfield", "save") else None)) + "\n")
                        if kind == "capture":
                            window.screenshot(directory / (values[0] + ".png"))
                        elif kind == "resize":
                            window.resize(*(self.large_size if values == [1200, 820] else values))
                        elif kind == "key":
                            window.key(values[0], control=False)
                        elif kind in ("wait_text", "wait_scroll_text", "wait_scroll_above", "wait_scroll_up", "wait_models"):
                            if kind in ("wait_scroll_text", "wait_scroll_above", "wait_scroll_up"):
                                self.wait_scrolled(window, directory, values[0], values[1] if len(values) > 1 else None,
                                    above_anchor=kind == "wait_scroll_above", up=kind == "wait_scroll_up")
                            else:
                                timeout = MODEL_CATALOG_TIMEOUT if kind == "wait_models" else 15
                                wait_visible(window, directory, values[0], timeout=timeout)
                        elif kind == "reveal_text":
                            point = self.scroll_anchor(window, directory, values[1])
                            wait_visible(window, directory, values[0], scroll=lambda: self.reveal_scroll(window, point))
                        elif kind == "sleep":
                            time.sleep(values[0])
                        elif kind == "wheel":
                            self.scroll(window, values[0])
                        else:
                            self.interact(window, directory, kind, values)
                if companion is not None:
                    stop_companion(self.profile)
                if self.session == "macos":
                    state = OwnedMacInput(process).release_modifiers()
                    (directory / "exit-input-state.json").write_text(json.dumps(state), encoding="utf-8")
                window.key("q")
                try:
                    exit_code = process.wait(timeout=20)
                except subprocess.TimeoutExpired as failure:
                    if self.session == "macos":
                        # Keep the owned window and native stacks when input or
                        # application shutdown stalls on a disposable runner.
                        for diagnose in (
                            lambda: window.screenshot(directory / "exit-timeout.png"),
                            lambda: run("sample", process.pid, 2, 1, "-file", directory / "exit-timeout.sample.txt", timeout=10),
                        ):
                            try:
                                diagnose()
                            except Exception as diagnostic_error:
                                failure.add_note(f"Exit diagnostic failed: {type(diagnostic_error).__name__}")
                    raise
                if exit_code != 0:
                    raise RuntimeError("Installed settings app failed on exit")
                if companion is not None and companion.wait(timeout=30) != 0:
                    raise RuntimeError("Electron config companion failed")
            finally:
                for child in (companion, process):
                    if child is not None and child.poll() is None:
                        child.kill()
                        child.wait(timeout=10)

    def scroll_anchor(self, window, directory, text, *, above=False):
        image = directory / "settings-scroll-anchor.png"
        label = self.locate(window, image, "click", text)
        scale = 1
        if self.session == "macos":
            scale = json.loads(image.with_suffix(".window.json").read_text())["capturePixelsPerWindowPoint"]
        x = (label["left"] + label["width"] / 2) / scale
        # The session pager stays outside the scrolling pane. Two measured
        # glyph heights above it are inside that pane, even after its content
        # has moved. Never wheel over the button or rely on request-header OCR.
        y = (label["top"] - 2 * label["height"] if above else label["top"] + label["height"] / 2) / scale
        if not (math.isfinite(x) and math.isfinite(y) and x >= 0 and y >= 0 and label["height"] > 0):
            raise RuntimeError("The measured scroll anchor is outside the owned capture")
        return x, y

    def reveal_scroll(self, window, point):
        # One Mac line left the compact cancellation status clipped, and slow
        # OCR exhausted the wait before a second correction. Keep each event
        # small, but complete both lines before inspecting the next frame.
        for _ in range(2 if self.session == "macos" else 1):
            self.scroll(window, -120, point)

    def wait_scrolled(self, window, directory, text, anchor=None, *, above_anchor=False, up=False):
        if above_anchor and anchor is None:
            raise RuntimeError("Scrolling above a control requires its measured label")
        point = None

        def move():
            nonlocal point
            if anchor is not None and point is None:
                point = (self.scroll_anchor(window, directory, anchor, above=True) if above_anchor
                    else self.scroll_anchor(window, directory, anchor))
            if point is None:
                self.scroll(window, 1200 if up else -1200)
            else:
                # Measure the owning content, retaining the ordinary coarse
                # step. Fine reveal gestures are for nearby chat feedback.
                # A Mac OCR miss can consume most of the wait. Reach the
                # bounded request page's end before inspecting another frame.
                for _ in range(2 if above_anchor and self.session == "macos" else 1):
                    self.scroll(window, 1200 if up else -1200, point, precise=False)

        wait_visible(window, directory, text, scroll=move)

    def scroll(self, window, delta, point=None, *, precise=None):
        if self.session == "x11":
            x, y = point if point is not None else (600, 400)
            run("xdotool", "mousemove", "--window", window.window, int(x), int(y))
            run("xdotool", "click", "--repeat", max(1, abs(delta) // 120), "--delay", 50, 5 if delta < 0 else 4)
        else:
            metadata = json.loads(run(Path(os.environ["RUNNER_TEMP"]) / "buddy-window-info", window.process.pid))
            bounds = metadata["kCGWindowBounds"]
            x, y = point if point is not None else (bounds["Width"] / 2, bounds["Height"] / 2)
            OwnedMacInput(window.process).scroll(bounds["X"] + x, bounds["Y"] + y, delta,
                precise=(point is not None) if precise is None else precise)
        time.sleep(0.7)

    def locate(self, window, image, kind, text, row_control=None):
        previous = None
        for _ in range(4 if self.session == "macos" else 1):
            window.screenshot(image)
            options = dict(navigation=kind == "nav",
                field=kind in ("input", "field", "box", "leftfield", "rightfield", "save"),
                save_row=self.session == "macos" and kind in ("field", "save"), tab=kind == "tab")
            if row_control is not None:
                options["row_control"] = row_control
            label = ocr.locate_image(image, text, **options)
            if self.session != "macos":
                return label
            record_measurement(image.parent, text, label)
            # A pending save can insert feedback while OCR runs, moving every
            # content control. Measure twice before emitting any Mac input.
            if stable(label, previous):
                return label
            previous = label
        raise RuntimeError("The visible Mac input target did not become stable")

    def input_receipt(self, helper, directory, stage, kind, label, target):
        with (directory / "input-state.jsonl").open("a", encoding="utf-8") as receipt:
            value = dict(stage=stage, kind=kind, label=label, target=target, state=helper.focus_state())
            trace = getattr(helper, "last_click_trace", None)
            if isinstance(trace, list):
                value["clickTrace"] = trace
            receipt.write(json.dumps(value) + "\n")

    def interact_cell(self, window, directory, values):
        if len(values) != 2 or not all(isinstance(value, str) and value for value in values):
            raise RuntimeError("A cell needs its visible row and date labels")
        grid = load("tempo-grid")
        image = directory / "cell-locator.png"
        previous = None
        for _ in range(4 if self.session == "macos" else 1):
            window.screenshot(image)
            point = grid.locate_image(image, *values)
            if self.session != "macos":
                break
            record_measurement(image.parent, " / ".join(values), point)
            if stable(point, previous):
                break
            previous = point
        else:
            raise RuntimeError("The visible grid cell did not become stable")
        x, y = point["x"], point["y"]
        if self.session == "macos":
            import struct
            width, height = struct.unpack(">II", image.read_bytes()[16:24])
            metadata = json.loads(image.with_suffix(".window.json").read_text())
            bounds, scale = metadata["kCGWindowBounds"], metadata["capturePixelsPerWindowPoint"]
            if (type(scale) not in (int, float) or not math.isfinite(scale) or scale <= 0
                    or not all(type(bounds.get(key)) in (int, float) and math.isfinite(bounds[key]) for key in ("X", "Y", "Width", "Height"))
                    or bounds["Width"] <= 0 or bounds["Height"] <= 0
                    or abs(width - bounds["Width"] * scale) > 1
                    or abs(height - bounds["Height"] * scale) > 1):
                raise RuntimeError("The grid capture does not match its owned window bounds")
            target = [bounds["X"] + x / scale, bounds["Y"] + y / scale]
            run("osascript", "-e", window.apple("set frontmost to true"))
            helper = OwnedMacInput(window.process)
            try:
                helper.click(*target)
            except Exception as failure:
                try:
                    self.input_receipt(helper, directory, "failed-click", "cell", values, target)
                    window.screenshot(directory / "failed-click.png")
                except Exception as diagnostic_error:
                    failure.add_note(f"Input diagnostic failed: {type(diagnostic_error).__name__}")
                raise
            self.input_receipt(helper, directory, "after-click", "cell", values, target)
        else:
            run("xdotool", "windowactivate", "--sync", window.window)
            run("xdotool", "mousemove", "--window", window.window, int(x), int(y), "click", 1)
        time.sleep(0.7)

    def interact(self, window, directory, kind, values):
        if kind == "cell":
            return self.interact_cell(window, directory, values)
        image = directory / "locator.png"
        hint = values[2] if kind == "input" and len(values) == 3 else None
        offset = 20
        if isinstance(hint, dict):
            if set(hint) != {"labelBottomOffset"} or type(hint["labelBottomOffset"]) is not int or not 20 <= hint["labelBottomOffset"] <= 40:
                raise RuntimeError("The input needs a bounded label-to-editor offset")
            offset = hint["labelBottomOffset"]
            row_control = None
        else:
            row_control = hint
        if row_control is None:
            label = self.locate(window, image, kind, values[0])
        else:
            label = self.locate(window, image, kind, values[0], row_control=row_control)
        if kind == "box":
            # A text box with no Save button in its row (Bookmarks' search):
            # located without the Mac Save-row anchor, then aimed and typed
            # like a field, at the middle of the window width under its label.
            kind = "field"
        import struct
        width, height = struct.unpack(">II", image.read_bytes()[16:24])
        scale, origin = 1, (0, 0)
        if self.session == "macos":
            metadata = json.loads(image.with_suffix(".window.json").read_text())
            bounds = metadata["kCGWindowBounds"]
            scale = metadata["capturePixelsPerWindowPoint"]
            origin = (bounds["X"], bounds["Y"])
        x = (label["left"] + label["width"] / 2) / scale
        y = (label["top"] + label["height"] / 2) / scale
        if kind in ("input", "field", "leftfield", "rightfield", "save"):
            y = (label["top"] + label["height"]) / scale + 20
            x = width / scale - 49 if kind == "save" else width / scale / 2
            if kind == "leftfield":
                x = width / scale / 4
            elif kind == "rightfield":
                x = width / scale * 3 / 4
            elif kind == "input":
                # Compact fields and spaced forms have different label gaps.
                # Only an explicit form hint may change the default target.
                x = label["left"] / scale + 20
                y = (label["top"] + label["height"]) / scale + offset
        if "row_control" in label:
            control = label["row_control"]
            y = (control["top"] + control["height"] / 2) / scale
        if "save" in label:
            # Label glyph bottoms do not locate the editor: on Mac the old
            # offset hit the frame's padding, which shows a focus ring while
            # leaving its text editor unfocused. Use the rendered row instead.
            control = label["save"]
            y = (control["top"] + control["height"] / 2) / scale
            if kind == "save":
                x = (control["left"] + control["width"] / 2) / scale
        if self.session == "macos":
            with (directory / "targets.jsonl").open("a", encoding="utf-8") as targets:
                targets.write(json.dumps(dict(kind=kind, label=values[0], bounds=label,
                    windowPoint=[x, y])) + "\n")
            run("osascript", "-e", window.apple("set frontmost to true"))
            helper = OwnedMacInput(window.process)
            target = [origin[0] + x, origin[1] + y]
            try:
                helper.click(*target)
            except Exception as failure:
                try:
                    self.input_receipt(helper, directory, "failed-click", kind, values[0], target)
                    window.screenshot(directory / "failed-click.png")
                except Exception as diagnostic_error:
                    failure.add_note(f"Input diagnostic failed: {type(diagnostic_error).__name__}")
                raise
            self.input_receipt(helper, directory, "after-click", kind, values[0], target)
        else:
            run("xdotool", "windowactivate", "--sync", window.window)
            run("xdotool", "mousemove", "--window", window.window, int(x), int(y), "click", 1)
        time.sleep(0.4)
        if kind in ("input", "field", "leftfield", "rightfield"):
            if self.session == "macos":
                helper = OwnedMacInput(window.process)
                if kind == "field" and values[0] == "Accent color":
                    (directory / "accent-focus-before.json").write_text(json.dumps(helper.focus_state()), encoding="utf-8")
                helper.replace_text(values[1])
                self.input_receipt(helper, directory, "after-typing", kind, values[0], target)
            else:
                window.key("a")
                if values[1].isascii():
                    run("xdotool", "type", "--clearmodifiers", "--delay", 10, "--", values[1])
                else:
                    with unicode_clipboard(values[1]):
                        window.key("v")
                        time.sleep(0.7)
        time.sleep(0.7)
        if self.session == "macos" and kind == "field" and values[0] == "Accent color":
            name = "valid" if values[1].startswith("#") else "invalid"
            window.screenshot(directory / f"typed-{name}-accent-draft.png")
            (directory / f"accent-focus-after-{name}.json").write_text(
                json.dumps(OwnedMacInput(window.process).focus_state()), encoding="utf-8")
        if self.session == "macos" and kind == "field" and values[0] == "Notification audio file":
            # Leave the editor and its adjacent Save button unfocused. Save's
            # focus outline hides its OCR glyphs, which locate the input row.
            window.key("Tab", control=False)
            window.key("Tab", control=False)
            image = directory / "typed-audio-draft.png"
            window.screenshot(image)
            (directory / "audio-input-state.json").write_text(
                json.dumps(OwnedMacInput(window.process).focus_state()), encoding="utf-8")
            verify_draft(image, values[0], values[1])


class MissingVisibleText(RuntimeError):
    """An OCR result lacks required text; engine and I/O errors remain distinct."""


def verify_text(path, *markers, mode=11):
    text = run("tesseract", path, "stdout", "--psm", mode, encoding="utf-8").casefold()
    path.with_suffix(".ocr.txt").write_text(text, encoding="utf-8")
    path.with_suffix(f".ocr-{mode}.txt").write_text(text, encoding="utf-8")
    # OCR wraps one paragraph onto several lines; compare words, not breaks.
    words = " ".join(text.split())
    for marker in markers:
        if " ".join(marker.casefold().split()) not in words:
            raise MissingVisibleText(f"Settings screenshot is missing {marker!r}")


def verify_color(path, expected, session):
    if session != "macos":
        verify_text(path, expected)
        return
    import struct
    label = ocr.locate_image(path, "Accent color", field=True, save_row=True)
    metadata = json.loads(path.with_suffix(".window.json").read_text())
    region = ocr.color_region(label, metadata["capturePixelsPerWindowPoint"],
        struct.unpack(">II", path.read_bytes()[16:24]))
    # OCR analysis only. The original screenshot remains the review evidence.
    cropped = path.with_suffix(".color-ocr.png")
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", f'crop={region["width"]}:{region["height"]}:{region["left"]}:{region["top"]}:exact=1,scale=iw*3:ih*3:flags=neighbor',
        "-frames:v", "1", cropped)
    for mode in (7, 13):  # Single text line and raw line; no glyph substitutions.
        actual = run("tesseract", cropped, "stdout", "--psm", mode,
            "-c", "tessedit_char_whitelist=#0123456789abcdef").strip().casefold()
        path.with_suffix(f".color-ocr-{mode}.txt").write_text(actual, encoding="utf-8")
        if actual == expected.casefold():
            return
    raise RuntimeError("The restarted color input does not show the exact saved hex value")


def verify_draft(path, label, expected):
    """Verify controlled fixture drafts without altering review screenshots."""
    import struct
    bounds = ocr.locate_image(path, label, field=True, save_row=True)
    region = ocr.input_region(bounds, struct.unpack(">II", path.read_bytes()[16:24]))
    analysis = path.with_suffix(".input-ocr.png")
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", f'crop={region["width"]}:{region["height"]}:{region["left"]}:{region["top"]}:exact=1,scale=iw*3:ih*3:flags=neighbor',
        "-frames:v", "1", analysis)
    for mode in (7, 13):
        actual = run("tesseract", analysis, "stdout", "--psm", mode).strip().casefold()
        path.with_suffix(f".input-ocr-{mode}.txt").write_text(actual, encoding="utf-8")
        if actual == expected.casefold():
            return
    raise RuntimeError("The input does not show the exact retained draft")


def verify_feedback_scope(path):
    text = " ".join(run("tesseract", path, "stdout", "--psm", 11).casefold().split())
    if "choose an absolute" in text:
        raise RuntimeError("Notifications validation feedback leaked into another panel")


def qualify(executable, output, session, checkout, backend):
    config = fixture(output, "dark")
    initial = json.loads(config.read_text())
    initial["native"]["activeSection"] = "settings"
    initial["ui"]["dashboardCards"] = {key: False for key in initial["ui"]["dashboardCards"]}
    config.write_text(json.dumps(initial))
    (output / ".buddy-settings-fixture").write_text("buddy settings fixture v1")
    run("bun", checkout / "scripts/build-config-companion.ts", output, cwd=checkout)
    driver = Driver(executable, output, session, backend, checkout)
    account_actions = [("nav", "Accounts"), ("capture", "first-run-dark")]
    for index, name in enumerate(("fixture-native-one", "fixture-native-two")):
        account_actions += [("leftfield", "Organization|GitHub username", name),
            ("rightfield", "Organization|GitHub username", "fixture-native-team"), ("click", "Add account"), ("sleep", 1)]
        if index == 0:
            account_actions += [("click", "Codex"), ("sleep", 1)]
    account_actions += [("capture", "two-accounts-dark")]
    accounts = driver.drive("accounts", account_actions, record=True)
    actual = backend.query("githubAccounts:list")
    if sorted(account["username"] for account in actual) != ["fixture-native-one", "fixture-native-two"]:
        raise RuntimeError("Native inputs did not create exactly two fixture accounts")
    if next(account for account in actual if account["username"] == "fixture-native-one").get("usageProvider") != "codex":
        raise RuntimeError("Provider selection did not reach the isolated backend")
    verify_text(accounts / "two-accounts-dark.png", "fixture-native-one", "fixture-native-two")
    appearance = driver.drive("appearance", [("nav", "Appearance"), ("click", "Light"),
        ("field", "Accent color", "#8246af"), ("save", "Accent color"), ("capture", "appearance-light"),
        ("field", "Accent color", "not-a-color"), ("save", "Accent color"), ("capture", "invalid-color-light"),
        ("resize", 900, 650), ("key", "Tab"), ("capture", "keyboard-compact-light")], concurrent=True, record=True)
    saved = json.loads(config.read_text(encoding="utf-8-sig"))
    receipt = current_companion(output, "complete")
    ready = current_companion(output, "ready")
    captured = (appearance / "appearance-light.png").stat().st_mtime * 1000
    verify_companion_overlap(ready, receipt, captured)
    if saved["ui"]["accentColor"] != "#8246af" or saved["ui"]["enterpriseSlug"] != "fixture-concurrent-complete" or receipt["writes"] < 2:
        if session == "macos":
            # Preserve the failed concurrent result. These bounded comparisons
            # collect a cause; they cannot turn the original failure into a pass.
            (output / "failed-concurrent-config.json").write_text(json.dumps(saved), encoding="utf-8")
            comparison = {}
            for name, theme_actions, expected in (
                ("with-theme-change", [("click", "Dark"), ("click", "Light")], "#51a26b"),
                ("without-theme-change", [], "#a1624b")):
                try:
                    driver.drive("input-diagnostic-" + name, [("nav", "Appearance"), *theme_actions,
                        ("field", "Accent color", expected), ("save", "Accent color"),
                        ("capture", "standalone-accent")])
                    comparison[name] = dict(saved=json.loads(config.read_text(encoding="utf-8-sig"))["ui"]["accentColor"] == expected)
                except Exception as diagnostic_error:
                    comparison[name] = dict(errorType=type(diagnostic_error).__name__)
            (output / "mac-input-comparison.json").write_text(json.dumps(comparison), encoding="utf-8")
        raise RuntimeError("Concurrent real Electron/native saves lost an edit")
    if saved["packageQualificationSentinel"] != {"preserve": True}:
        raise RuntimeError("Unknown shared config fields were lost")
    verify_text(appearance / "keyboard-compact-light.png", "six-digit hex color")
    preferences = driver.drive("preferences", [("nav", "Pull requests"), ("click", "10"), ("wait_text", "Saved"),
        ("click", "Automatic refresh: on"), ("wait_text", "Saved"), ("capture", "pr-light"), ("nav", "Copilot"),
        ("field", "GitHub CLI account", "fixture-native-one"), ("save", "GitHub CLI account"), ("wait_text", "Saved"),
        ("field", "Model ID|Model 1D|Model lD", "fixture-model"),
        ("save", "Model ID|Model 1D|Model lD"), ("wait_text", "Saved"), ("capture", "copilot-light"),
        ("nav", "Notifications"), ("click", "Review completion sound: off"), ("wait_text", "Review completion sound: on"),
        ("field", "Notification audio file", "relative.wav"), ("save", "Notification audio file"),
        ("capture", "invalid-audio-light"), ("nav", "Weather"), ("capture", "weather-status-light"),
        ("nav", "Advanced"), ("capture", "advanced-light")])
    verify_text(preferences / "invalid-audio-light.png", "absolute")
    for name in ("weather-status-light", "advanced-light"):
        verify_feedback_scope(preferences / (name + ".png"))
    snapshot = backend.query("settings:get")
    if snapshot["pr"]["refreshInterval"] != 10 or snapshot["pr"]["autoRefresh"] is not False or snapshot["copilot"]["model"] != "fixture-model":
        raise RuntimeError("Synced settings did not reach Convex")
    restarted = driver.drive("restart", [("nav", "Appearance"), ("capture", "persisted-light"),
        ("resize", 900, 650), ("capture", "persisted-compact-light"), ("click", "Dark"),
        ("capture", "persisted-compact-dark"), ("resize", 1200, 820), ("nav", "Accounts"),
        ("capture", "accounts-after-restart-dark")])
    verify_color(restarted / "persisted-light.png", "#8246af", session)
    verify_text(restarted / "accounts-after-restart-dark.png", "fixture-native-one", "fixture-native-two")
    font = "DejaVu Sans" if session == "x11" else "Arial"
    mono = "DejaVu Sans Mono" if session == "x11" else "Courier New"
    driver.drive("appearance-controls", [("nav", "Appearance"), ("wheel", -2400),
        ("capture", "fonts-and-dashboard-dark"), ("field", "Interface font", font), ("save", "Interface font"),
        ("field", "Monospace font", mono), ("save", "Monospace font"), ("wheel", -2400),
        ("click", "Command Center: hidden"), ("capture", "applied-fonts-dashboard-dark")])
    saved = json.loads(config.read_text(encoding="utf-8-sig"))
    if saved["ui"]["fontFamily"] != font or saved["ui"]["monoFontFamily"] != mono or not saved["ui"]["dashboardCards"]["command-center"]:
        raise RuntimeError("Font and dashboard visibility edits did not persist")
    drafts = driver.drive("retained-drafts", [("nav", "Appearance"), ("wheel", -2400),
        ("field", "Interface font", "Draft font"), ("nav", "Accounts"), ("wheel", -2400),
        ("field", "GitHub enterprise slug", "draft-enterprise"), ("nav", "Notifications"), ("wheel", 2400),
        ("field", "Notification audio file", "draft audio"), ("key", "Tab"),
        ("nav", "Appearance"), ("wheel", 2400),
        ("click", "Light"), ("click", "Dark"), ("wheel", -2400),
        ("capture", "theme-retained-font-dark"), ("nav", "Accounts"), ("wheel", -2400),
        ("capture", "theme-retained-enterprise-dark"), ("nav", "Notifications"), ("wheel", 2400),
        ("capture", "theme-retained-audio-dark")], record=True)
    for name, label, expected in (("font", "Interface font", "Draft font"),
            ("enterprise", "GitHub enterprise slug", "draft-enterprise"),
            ("audio", "Notification audio file", "draft audio")):
        verify_draft(drafts / f"theme-retained-{name}-dark.png", label, expected)
    saved = json.loads(config.read_text(encoding="utf-8-sig"))
    if saved["ui"]["fontFamily"] != font or saved["ui"]["enterpriseSlug"] != "fixture-concurrent-complete" or saved["notifications"]["reviewCompleteSoundPath"]:
        raise RuntimeError("Theme changes committed an unrelated unsaved draft")
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", session=session, largeWindow=driver.large_size,
        binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest(), checks=["two native accounts", "provider selection", "all seven panels",
        "real Electron/native concurrent writes", "unknown fields", "invalid color/audio", "Convex preferences", "restart", "retained unrelated drafts", "light/dark", "keyboard Tab", f"{driver.large_size[0]}x{driver.large_size[1]}/900x650"]), indent=2))


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Settings qualification requires disposable GitHub Actions fixtures")
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    args = parser.parse_args()
    if args.output.exists():
        raise RuntimeError("Settings evidence requires a fresh output directory")
    checkout = HERE.parent.parent
    with SettingsBackend(checkout) as backend:
        qualify(args.executable.resolve(strict=True), args.output.resolve(), args.session, checkout, backend)


if __name__ == "__main__":
    main()
