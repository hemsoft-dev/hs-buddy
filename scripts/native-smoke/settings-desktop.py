"""Qualify real installed settings against a disposable anonymous backend."""
import argparse
import base64
from contextlib import nullcontext
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from desktop import UnixWindow, encode_recording, fixture, recording, run


HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


ocr = load("settings-ocr")
SettingsBackend = load("settings-backend").SettingsBackend


def b64(value):
    return base64.b64encode(str(value).encode()).decode()


class Driver:
    def __init__(self, executable, profile, session, backend, checkout):
        self.executable, self.profile, self.session = executable, profile, session
        self.backend, self.checkout = backend, checkout
        self.config = profile / "config.json"
        self.electron = run("bun", "-e", "console.log(require('electron'))", cwd=checkout).splitlines()[-1]

    def drive(self, name, actions, concurrent=False, record=False):
        directory = self.profile / name
        directory.mkdir()
        if self.session == "windows":
            commands = ["resize:80:40:1200:820"]
            if record:
                commands += ["record-start:" + str(directory / "frames")]
            for action in actions:
                kind, *values = action
                if kind in ("nav", "click", "field", "leftfield", "rightfield", "save"):
                    commands.append(kind + "64:" + ":".join(map(b64, values)))
                elif kind == "capture":
                    commands.append("capture:" + str(directory / (values[0] + ".png")))
                elif kind == "resize":
                    commands.append(f"resize:80:40:{values[0]}:{values[1]}")
                elif kind == "key":
                    commands.append("key:" + str(dict(Tab=9, Return=13, Escape=27)[values[0]]))
                elif kind == "sleep":
                    commands.append("sleep:" + str(int(values[0] * 1000)))
                else:
                    raise RuntimeError("Unknown settings action")
            if record:
                commands.append("record-stop")
            commands.append("ctrlkey:81")
            arguments = ["pwsh", "-NoProfile", "-File", HERE / "windows-window.ps1", "-Exe", self.executable,
                "-OutDir", directory, "-ConfigPath", self.config, "-ConvexUrl", self.backend.url, "-Actions", ",".join(commands)]
            if concurrent:
                arguments += ["-CompanionExe", self.electron, "-CompanionScript", self.profile]
            run(*arguments, cwd=self.checkout, timeout=180)
        else:
            self.unix(directory, actions, concurrent, record)
        if record:
            encode_recording(directory)
        return directory

    def unix(self, directory, actions, concurrent, record):
        environment = dict(os.environ, BUDDY_CONFIG_PATH=str(self.config), BUDDY_CONVEX_URL=self.backend.url,
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
                window.resize(1200, 820)
                if concurrent:
                    companion = subprocess.Popen([self.electron, str(self.profile)], cwd=self.profile,
                        env=environment, stdout=log, stderr=log)
                    deadline = time.monotonic() + 20
                    while not (self.profile / "companion-ready.json").exists():
                        if companion.poll() is not None or time.monotonic() > deadline:
                            raise RuntimeError("Electron config companion did not become ready")
                        time.sleep(0.1)
                with recording(window, directory / "frames") if record else nullcontext():
                    for kind, *values in actions:
                        if kind == "capture":
                            window.screenshot(directory / (values[0] + ".png"))
                        elif kind == "resize":
                            window.resize(*values)
                        elif kind == "key":
                            window.key(values[0], control=False)
                        elif kind == "sleep":
                            time.sleep(values[0])
                        else:
                            self.interact(window, directory, kind, values)
                window.key("q")
                if process.wait(timeout=20) != 0:
                    raise RuntimeError("Installed settings app failed on exit")
                if companion is not None and companion.wait(timeout=30) != 0:
                    raise RuntimeError("Electron config companion failed")
            finally:
                for child in (companion, process):
                    if child is not None and child.poll() is None:
                        child.kill()
                        child.wait(timeout=10)

    def interact(self, window, directory, kind, values):
        image = directory / "locator.png"
        window.screenshot(image)
        label = ocr.locate_label(ocr.recognize(image), values[0], navigation=kind == "nav",
            field=kind in ("field", "leftfield", "rightfield", "save"))
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
        if kind in ("field", "leftfield", "rightfield", "save"):
            y = (label["top"] + label["height"]) / scale + 20
            x = width / scale - 49 if kind == "save" else width / scale / 2
            if kind == "leftfield":
                x = width / scale / 4
            elif kind == "rightfield":
                x = width / scale * 3 / 4
        if self.session == "macos":
            run("osascript", "-e", window.apple(f"set frontmost to true\nclick at {{{int(origin[0]+x)}, {int(origin[1]+y)}}}"))
        else:
            run("xdotool", "windowactivate", "--sync", window.window)
            run("xdotool", "mousemove", "--window", window.window, int(x), int(y), "click", 1)
        time.sleep(0.4)
        if kind in ("field", "leftfield", "rightfield"):
            window.key("a")
            if self.session == "macos":
                run("osascript", "-e", window.apple("keystroke " + json.dumps(values[1])))
            else:
                run("xdotool", "type", "--clearmodifiers", "--delay", 10, "--", values[1])
        time.sleep(0.7)


def verify_text(path, *markers):
    text = run("tesseract", path, "stdout", "--psm", 11).casefold()
    path.with_suffix(".ocr.txt").write_text(text, encoding="utf-8")
    for marker in markers:
        if marker.casefold() not in text:
            raise RuntimeError(f"Settings screenshot is missing {marker!r}")


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
    receipt = json.loads((output / "companion-complete.json").read_text())
    ready = json.loads((output / "companion-ready.json").read_text())
    captured = (appearance / "appearance-light.png").stat().st_mtime * 1000
    if not ready["startedAt"] <= captured <= receipt["completedAt"]:
        raise RuntimeError("Native save was not observed during actual Electron writes")
    if saved["ui"]["accentColor"] != "#8246af" or saved["ui"]["enterpriseSlug"] != "fixture-concurrent-complete" or receipt["writes"] < 2:
        raise RuntimeError("Concurrent real Electron/native saves lost an edit")
    if saved["packageQualificationSentinel"] != {"preserve": True}:
        raise RuntimeError("Unknown shared config fields were lost")
    verify_text(appearance / "invalid-color-light.png", "six-digit hex color")
    driver.drive("preferences", [("nav", "Pull requests"), ("click", "5"),
        ("click", "Automatic refresh: on"), ("capture", "pr-light"), ("nav", "Copilot"),
        ("field", "GitHub CLI account", "fixture-native-one"), ("save", "GitHub CLI account"),
        ("field", "Model ID", "fixture-model"), ("save", "Model ID"), ("capture", "copilot-light"),
        ("nav", "Notifications"), ("click", "Review completion sound: off"),
        ("field", "Notification audio file", "relative.wav"), ("save", "Notification audio file"),
        ("capture", "invalid-audio-light"), ("nav", "Weather"), ("capture", "weather-status-light"),
        ("nav", "Advanced"), ("capture", "advanced-light")])
    snapshot = backend.query("settings:get")
    if snapshot["pr"]["refreshInterval"] != 5 or snapshot["pr"]["autoRefresh"] is not False or snapshot["copilot"]["model"] != "fixture-model":
        raise RuntimeError("Synced settings did not reach Convex")
    restarted = driver.drive("restart", [("nav", "Appearance"), ("capture", "persisted-light"),
        ("resize", 900, 650), ("capture", "persisted-compact-light"), ("click", "Dark"),
        ("capture", "persisted-compact-dark"), ("resize", 1200, 820), ("nav", "Accounts"),
        ("capture", "accounts-after-restart-dark")])
    verify_text(restarted / "persisted-light.png", "#8246af")
    verify_text(restarted / "accounts-after-restart-dark.png", "fixture-native-one", "fixture-native-two")
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", session=session,
        binarySha256=hashlib.sha256(executable.read_bytes()).hexdigest(), checks=["two native accounts", "provider selection", "all seven panels",
        "real Electron/native concurrent writes", "unknown fields", "invalid color/audio", "Convex preferences", "restart", "light/dark", "keyboard Tab", "1200x820/900x650"]), indent=2))


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
