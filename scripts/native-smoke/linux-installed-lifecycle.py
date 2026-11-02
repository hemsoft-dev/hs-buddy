"""Real deb installation and runtime rollback; never a native-default cutover."""
import argparse
import contextlib
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import signal
import socket
import subprocess
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from threading import Thread
import time
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
PROJECT = "lifecycle-project"
MESSAGE = "Retained lifecycle conversation and recovery"
CACHE_KEY = "pr:lifecycle-fixture"
MARKER = "installed-lifecycle-marker"
FRESH_DASHBOARD_BODY = ("Customize", "Your Universal Productivity Companion")


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


LAYOUT = load(HERE / "linux-lifecycle-layout.py")


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def profile_manifest(profile):
    result = {}
    for directory, _, files in os.walk(profile, followlinks=False):
        for name in files:
            path = Path(directory) / name
            if not path.is_symlink() and path.is_file():
                result[path.relative_to(profile).as_posix()] = dict(bytes=path.stat().st_size, sha256=digest(path))
    return result


def require_runner(output):
    if (os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1"
            or platform.machine() != "x86_64"):
        raise RuntimeError("Installed lifecycle requires an owned Ubuntu x64 CI runner")
    release_text = Path("/etc/os-release").read_text(encoding="utf-8")
    release = dict(line.split("=", 1) for line in release_text.splitlines() if "=" in line)
    if release.get("ID", "").strip('"') != "ubuntu" or release.get("VERSION_ID", "").strip('"') != "24.04":
        raise RuntimeError("Installed lifecycle qualifies Ubuntu 24.04 only")
    root = Path(os.environ["RUNNER_TEMP"]).resolve()
    if output.exists() or not output.resolve().is_relative_to(root) or output.resolve() == root:
        raise RuntimeError("A fresh evidence directory under RUNNER_TEMP is required")
    if not os.environ.get("DBUS_SESSION_BUS_ADDRESS") or not os.environ.get("DISPLAY"):
        raise RuntimeError("The isolated Secret Service and X11 session are required")
    return release_text


def require_subset(expected, actual, label="data"):
    """Allow app-added defaults, never a missing or changed seeded value."""
    if isinstance(expected, dict):
        if not isinstance(actual, dict):
            raise RuntimeError(f"Lost {label}")
        for key, value in expected.items():
            if key not in actual:
                raise RuntimeError(f"Lost {label}.{key}")
            require_subset(value, actual[key], f"{label}.{key}")
    elif isinstance(expected, list):
        if not isinstance(actual, list) or len(expected) != len(actual):
            raise RuntimeError(f"Changed {label}")
        for index, value in enumerate(expected):
            require_subset(value, actual[index], f"{label}[{index}]")
    elif expected != actual or isinstance(expected, bool) != isinstance(actual, bool):
        raise RuntimeError(f"Changed {label}")


def electron_config_baseline(original, actual, *, layout=None, phase=None):
    """Measure known Electron normalizations before native uses this profile.

    This exception is never used for native or rollback comparisons. Preserve
    every other seeded key, including unknown keys in and outside accounts.
    """
    expected = copy.deepcopy(original)
    layout_changes = []
    if layout is not None or original.get("ui", {}).get("paneSizes") != actual.get("ui", {}).get("paneSizes"):
        layout_changes = LAYOUT.startup_normalization(original, actual, layout, phase)
        expected["ui"]["paneSizes"] = copy.deepcopy(actual["ui"]["paneSizes"])
    removals = []
    accounts = actual.get("github", {}).get("accounts", [])
    for index, account in enumerate(expected.get("github", {}).get("accounts", [])):
        if (index < len(accounts) and "futureAccount" in account
                and "futureAccount" not in accounts[index]):
            removals.append(dict(path=f"github.accounts[{index}].futureAccount",
                before=account.pop("futureAccount"), after="absent",
                cause="Electron connected account snapshot retains supported account fields only"))
    require_subset(expected, actual, "Electron startup config")
    return dict(config=copy.deepcopy(actual), observedRemovals=removals, layoutNormalization=layout_changes,
        layoutObservation=copy.deepcopy(layout), nativeWorkStarted=False, profileScope="shared synthetic lifecycle profile",
        unqualifiedBoundary="Unknown GitHub account extensions are not qualified for preservation by Electron's connected mirror")


def restored_state(value):
    history = value.get("session", {}).get("conversationHistory") or []
    if (value.get("session", {}).get("status") != "error" or not history
            or history[-1].get("content") != MESSAGE
            or value.get("cache", {}).get("data") != {"fixture": MARKER}
            or value.get("viewMode") != "list"):
        raise RuntimeError("The installed Electron app did not restore its local data through real IPC/storage")
    if not any(project.get("id") == PROJECT for project in value.get("projects", [])):
        raise RuntimeError("The installed Electron app lost its Crew project")


@contextlib.contextmanager
def browser_fixture():
    class Page(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            body = ("<!doctype html><title>Lifecycle browser</title><body style='font:24px monospace;color:#000;background:#fff'><h1>Lifecycle browser</h1>"
                "<script>if(location.search==='?seed=1'){localStorage.setItem('lifecycle','" + MARKER + "');"
                "document.cookie='lifecycle=" + MARKER + ";path=/;max-age=3600'}"
                "document.body.append(document.createTextNode(localStorage.getItem('lifecycle')||'missing'));"
                "</script>").encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Page)
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        # Electron correctly refuses private URL navigation. A process-local
        # HTTP proxy serves a documentation-only address from loopback instead;
        # no production URL validator or Chromium sandbox is disabled.
        yield dict(url=f"http://203.0.113.1:{server.server_port}/", proxy=f"http://127.0.0.1:{server.server_port}")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


class Packages:
    def __init__(self, output):
        self.output = output
        self.records = []
        self.owned = set()

    def record(self, **value):
        self.records.append(value)
        write(self.output / "packages.json", self.records)

    def status(self, name):
        result = subprocess.run(["dpkg-query", "-W", "-f=${db:Status-Status}", name],
            capture_output=True, text=True, timeout=10)
        if result.returncode not in (0, 1):
            raise RuntimeError("Could not inspect the package database")
        return result.stdout.strip() if result.returncode == 0 else "not-installed"

    def run(self, args, *, timeout=180, **kwargs):
        with (self.output / "commands.log").open("a", encoding="utf-8") as log:
            result = subprocess.run(args, stdout=log, stderr=log, timeout=timeout, **kwargs)
        if result.returncode:
            raise RuntimeError(f"Lifecycle command failed: {args[0]} exit {result.returncode}; see commands.log")

    def field(self, package, field):
        return subprocess.check_output(["dpkg-deb", "-f", str(package), field], text=True).strip()

    def install(self, package):
        package = package.resolve()
        name, version = self.field(package, "Package"), self.field(package, "Version")
        before = self.status(name)
        if before not in ("not-installed", "config-files"):
            raise RuntimeError("Preserve a pre-existing installed or partial package")
        # Ownership begins before apt, not after validation. Failed apt may
        # already have unpacked files; wrong-version validation can also fail
        # after a successful install. Neither may escape owned cleanup.
        self.record(action="install-attempt", package=name, version=version, priorState=before, sha256=digest(package))
        self.owned.add(name)
        try:
            self.run(["sudo", "apt-get", "install", "-y", "--no-install-recommends", "--allow-downgrades", str(package)])
            installed = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", name], text=True).strip()
            if installed != version or self.status(name) != "installed":
                raise RuntimeError("The installed deb version or state does not match its package")
        except BaseException as failure:
            self.record(action="install-failed", package=name, failureType=type(failure).__name__)
            raise
        self.record(action="install", package=name, version=version, sha256=digest(package))
        return name

    def remove(self, name, *, baseline=False):
        if name not in self.owned and not baseline:
            raise RuntimeError("Refuse cleanup of an unowned baseline package")
        # No autoremove, purge, profile deletion or modified package payloads.
        self.run(["sudo", "apt-get", "remove", "-y", name])
        if self.status(name) not in ("not-installed", "config-files"):
            raise RuntimeError("The package is still installed after removal")
        self.owned.discard(name)
        self.record(action="remove", package=name, baseline=baseline)


def open_browser_tab(cdp, page):
    # BookmarkList uses this route; shell.openInAppBrowser opens a separate
    # BrowserWindow, which cannot supply the main renderer's webview storage.
    route = "browser:" + quote(page, safe="") + "|" + quote("Lifecycle browser", safe="")
    cdp.evaluate("window.dispatchEvent(new CustomEvent('app:navigate',{detail:{viewId:" + json.dumps(route) + "}}))")


def open_crew_project(cdp):
    route = "crew-project:" + PROJECT
    cdp.evaluate("window.dispatchEvent(new CustomEvent('app:navigate',{detail:{viewId:" + json.dumps(route) + "}}))")


def crew_visible(cdp):
    return cdp.evaluate(f"[...document.querySelectorAll('h2')].some(h=>h.textContent==={json.dumps(PROJECT)}) && document.body.innerText.includes({json.dumps(MESSAGE)}) && document.body.innerText.includes('Session encountered an error')")


def capture_visible(visible, window, paired, process, directory, github, name, markers, *, browser=False):
    # DOM readiness can precede the compositor's painted frame. Require the
    # same strict markers in an owned-window capture within the existing wait.
    attempts = []
    receipt = directory / f"{name}-capture-attempts.json"
    write(receipt, attempts)

    def captured():
        if not visible():
            return False
        image = directory / f"{name}-captures" / f"{len(attempts):03}" / f"{name}-restored.png"
        attempt = dict(image=image.relative_to(directory).as_posix(), result="capture-error")
        attempts.append(attempt)
        try:
            image.parent.mkdir(parents=True)
            window.screenshot(image)
            try:
                github.check(image, *markers, **(dict(browser=True) if browser else {}))
            except RuntimeError as failure:
                if not isinstance(failure.__cause__, github.settings.MissingVisibleText):
                    attempt["result"] = "ocr-error"
                    raise
                attempt["result"] = "missing-visible-text"
                return False
            if not visible():
                attempt["result"] = "dom-changed"
                return False
            (directory / f"{name}-restored.png").write_bytes(image.read_bytes())
            (directory / f"{name}-restored.ocr.txt").write_bytes(image.with_suffix(".ocr.txt").read_bytes())
            attempt["result"] = "accepted"
            return True
        except OSError as failure:
            raise RuntimeError(f"Owned {name.title()} capture or evidence IO failed") from failure
        finally:
            try:
                write(receipt, attempts)
            except OSError as failure:
                raise RuntimeError(f"Owned {name.title()} capture receipt IO failed") from failure

    paired.wait_for(captured, process)



def capture_crew(cdp, window, paired, process, directory, github):
    capture_visible(lambda: crew_visible(cdp), window, paired, process, directory, github,
        "crew", (PROJECT, MESSAGE, "Session encountered an error"))


def capture_browser(cdp, window, paired, process, directory, github):
    def visible():
        state = browser_readback(cdp)
        write(directory / "browser-state.json", state)
        if state.get("marker") != MARKER or "lifecycle=" + MARKER not in state.get("cookie", ""):
            raise RuntimeError("The installed Electron browser did not restore its localStorage/cookie")
        return state.get("readyState") == "complete" and state.get("visibility") == "visible" and all(
            marker in state.get("text", "") for marker in ("Lifecycle browser", MARKER))
    capture_visible(visible, window, paired, process, directory, github,
        "browser", ("Lifecycle browser", MARKER), browser=True)


def browser_readback(cdp):
    # Keep guest state and actual host/guest geometry even if visible proof
    # fails. These diagnostics never substitute for the owned-window OCR.
    guest = """(() => {
        const r = document.body.getBoundingClientRect();
        const style = getComputedStyle(document.body);
        return {marker:localStorage.getItem('lifecycle'), cookie:document.cookie,
            text:document.body.innerText, url:location.href, readyState:document.readyState,
            visibility:document.visibilityState,
            viewport:{width:innerWidth,height:innerHeight,scale:devicePixelRatio},
            body:{x:r.x,y:r.y,width:r.width,height:r.height,
                color:style.color,background:style.backgroundColor}};
    })()"""
    return cdp.evaluate("""(async () => {
        const view = document.querySelector('webview');
        const state = await view.executeJavaScript(""" + json.dumps(guest) + """);
        const r = view.getBoundingClientRect();
        const style = getComputedStyle(view);
        return {...state,host:{url:view.getURL(),loading:view.isLoading(),
            bounds:{x:r.x,y:r.y,width:r.width,height:r.height},
            display:style.display,visibility:style.visibility,opacity:style.opacity}};
    })()""")


def app_environment(profile, backend):
    allowed = {"PATH", "DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR", "LANG", "LD_LIBRARY_PATH"}
    environment = {key: value for key, value in os.environ.items() if key in allowed}
    for name in ("home", "cache", "empty-gh"):
        (profile / name).mkdir(exist_ok=True, parents=True)
    environment.update(HOME=str(profile / "home"), XDG_CONFIG_HOME=str(profile.parent),
        XDG_CACHE_HOME=str(profile / "cache"), GH_CONFIG_DIR=str(profile / "empty-gh"),
        BUDDY_CONFIG_PATH=str(profile / "config.json"), BUDDY_CONVEX_URL=backend.url,
        BUDDY_ELECTRON_E2E="1", OTEL_SDK_DISABLED="true", GPUI_DISABLE_DIRECT_COMPOSITION="1")
    return environment


@contextlib.contextmanager
def running(executable, args, environment, directory, *, electron=False):
    directory.mkdir(parents=True)
    desktop = load(HERE / "desktop.py")
    with (directory / "app.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen([str(executable), *args], env=environment, cwd=ROOT,
            stdout=log, stderr=log, start_new_session=True)
        write(directory / "process.json", dict(pid=process.pid, executable=str(executable.resolve()),
            binarySha256=digest(executable), arguments=args, configPath=environment["BUDDY_CONFIG_PATH"],
            backendUrl=environment["BUDDY_CONVEX_URL"]))
        window = desktop.UnixWindow(process, "x11")
        deadline = time.monotonic() + 60
        try:
            while True:
                if process.poll() is not None:
                    raise RuntimeError("The installed application exited before its window was ready")
                try:
                    if window.ready():
                        break
                except RuntimeError:
                    pass
                if time.monotonic() >= deadline:
                    raise RuntimeError("The installed application did not show its owned window")
                time.sleep(.2)
            yield process, window
            if electron:
                # Linux Electron has no Ctrl+Q binding. Openbox's Alt+F4
                # requests a normal window close and preserves app shutdown.
                window.key("alt+F4", control=False)
            else:
                window.key("q")
            if process.wait(timeout=25) != 0:
                raise RuntimeError("The installed application did not exit cleanly")
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)


@contextlib.contextmanager
def recorded(window, directory):
    desktop = load(HERE / "desktop.py")
    with desktop.recording(window, directory / "frames"):
        yield
    desktop.encode_recording(directory)


def electron_pass(executable, profile, backend, output, name, browser, *, seed=False):
    # Reuse the production-package performance CDP client, not a fake renderer.
    paired = load(ROOT / "perf/native-comparison.py")
    github = load(HERE / "github-desktop.py")
    original_config = read(profile / "config.json")
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    environment = app_environment(profile, backend)
    environment["BUDDY_DEBUG_PORT"] = str(port)
    directory = output / name
    arguments = [f"--user-data-dir={profile}", f"--proxy-server={browser['proxy']}", "--proxy-bypass-list=127.0.0.1;localhost;[::1]"]
    with running(executable, arguments, environment, directory, electron=True) as (process, window), recorded(window, directory):
        import psutil
        cdp = paired.CDP(port, psutil.Process(process.pid))
        try:
            paired.wait_for(lambda: cdp.evaluate("window.__buddyPreloadReady === true && Boolean(document.querySelector('.activity-bar'))"), psutil.Process(process.pid))
            actual_path = cdp.evaluate("window.ipcRenderer.invoke('config:get-store-path')")
            if Path(actual_path).resolve() != (profile / "config.json").resolve():
                raise RuntimeError("Electron selected a profile outside the owned fixture")
            geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", window.window], text=True)
            dimensions = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
            if abs(int(dimensions["WIDTH"]) - 1200) > 2 or abs(int(dimensions["HEIGHT"]) - 820) > 2:
                raise RuntimeError("The installed Electron window did not restore its seeded layout")
            (directory / "restored-geometry.txt").write_text(geometry, encoding="utf-8")
            if seed:
                cdp.evaluate(f"window.crew.createSession({json.dumps(PROJECT)})")
                cdp.evaluate(f"window.crew.addMessage({json.dumps(PROJECT)}, {{role:'user',content:{json.dumps(MESSAGE)},timestamp:Date.now()}})")
                cdp.evaluate(f"window.crew.updateSessionStatus({json.dumps(PROJECT)}, 'error')")
                cdp.evaluate(f"window.ipcRenderer.invoke('cache:write', {json.dumps(CACHE_KEY)}, {{data:{{fixture:{json.dumps(MARKER)}}},fetchedAt:Date.now()}})")
                cdp.evaluate("localStorage.setItem('viewMode:pr-my-prs','list')")
            value = cdp.evaluate(f"(async()=>({{projects:await window.crew.listProjects(),session:await window.crew.getSession({json.dumps(PROJECT)}),cache:await window.ipcRenderer.invoke('cache:read',{json.dumps(CACHE_KEY)}),viewMode:localStorage.getItem('viewMode:pr-my-prs'),config:await window.ipcRenderer.invoke('config:get-config')}}))()")
            restored_state(value)
            write(directory / "restored-state.json", value)
            open_crew_project(cdp)
            capture_crew(cdp, window, paired, psutil.Process(process.pid), directory, github)
            page = browser["url"] + ("?seed=1" if seed else "")
            open_browser_tab(cdp, page)
            paired.wait_for(lambda: cdp.evaluate("(()=>{try{const w=document.querySelector('webview');return Boolean(w?.getURL().startsWith('http://203.0.113.1:'))&&!w.isLoading()}catch{return false}})()"), psutil.Process(process.pid))
            capture_browser(cdp, window, paired, psutil.Process(process.pid), directory, github)
            value["layout"] = LAYOUT.collect(cdp, psutil.Process(process.pid), paired.wait_for,
                original_config, directory, name, write)
            value["config"] = value["layout"]["samples"][-1]["config"]
            write(directory / "restored-state.json", value)
            return value
        finally:
            cdp.ws.close()


def check_fresh_dashboard_body(path, settings):
    """Require unconditional header controls in one captured body OCR layout."""
    missing = None
    for image in settings.ocr.analysis_images(path, settings.run):
        for mode in (11, 3, 6):
            words = settings.ocr.recognize(image, mode)
            write(image.with_suffix(f".dashboard-ocr-{mode}.json"), words)
            try:
                body = settings.ocr.content_words(words)
                for marker in FRESH_DASHBOARD_BODY:
                    settings.ocr.locate_label(body, marker)
            except settings.ocr.AmbiguousLabel:
                raise
            except RuntimeError as failure:
                missing = failure
                continue
            write(path.with_suffix(".dashboard-body.json"), dict(mode=mode, analysisImage=image.name,
                markers=list(FRESH_DASHBOARD_BODY), bodyWords=body))
            return
    raise RuntimeError("The fresh native screenshot does not show the dashboard body") from missing


def native_pass(executable, profile, backend, output, name, *, fresh=False):
    settings = load(HERE / "settings-desktop.py")
    if fresh:
        directory = output / name
        with running(executable, [], app_environment(profile, backend), directory) as (_, window):
            settings.wait_visible(window, directory, "Customize", content=True)
            window.screenshot(directory / "fresh.png")
            check_fresh_dashboard_body(directory / "fresh.png", settings)
        return
    github = load(HERE / "github-desktop.py")
    directory = output / name
    with running(executable, [], app_environment(profile, backend), directory) as (_, window):
        settings.wait_visible(window, directory, "No projects yet", content=True)
        window.screenshot(directory / "legacy-projects.png")
        geometry = subprocess.check_output(["xdotool", "getwindowgeometry", "--shell", window.window], text=True)
        dimensions = dict(line.split("=", 1) for line in geometry.splitlines() if "=" in line)
        if abs(int(dimensions["WIDTH"]) - 1200) > 2 or abs(int(dimensions["HEIGHT"]) - 820) > 2:
            raise RuntimeError("The installed native window did not restore the Electron layout")
        (directory / "restored-geometry.txt").write_text(geometry, encoding="utf-8")
        github.check(directory / "legacy-projects.png", "The Crew", "No projects yet", absent=(PROJECT,))
    return dict(crew="unmet: legacy project/conversation absent from native UI")


def build_electron(packages, backend):
    environment = dict(os.environ, VITE_CONVEX_URL=backend.url, CSC_IDENTITY_AUTO_DISCOVERY="false")
    environment.pop("GH_TOKEN", None)
    environment.pop("GITHUB_TOKEN", None)
    for command in (["node", "node_modules/electron/install.js"], ["bun", "x", "vite", "build"],
            ["bun", "x", "electron-builder", "--linux", "deb", "--x64", "--publish", "never", "-c.npmRebuild=false"]):
        packages.run(command, cwd=ROOT, env=environment, timeout=600)
    version = read(ROOT / "package.json")["version"]
    candidates = list((ROOT / "release" / version).glob("*.deb"))
    if len(candidates) != 1:
        raise RuntimeError("Expected one canonical Electron deb from the normal package builder")
    return candidates[0]


def config_fixture():
    fixture = read(ROOT / "rust/crates/buddy-core/tests/fixtures/electron-config.json")
    fixture["github"]["accounts"] = [dict(username="fixture-lifecycle", org="fixture-org", usageProvider="copilot", futureAccount=dict(preserve=True))]
    # An explicit connected provider avoids testing an interim local override
    # that normal account reconciliation intentionally clears.
    fixture["github"]["usageProviderOverrides"] = {}
    fixture["copilot"].update(ghAccount="fixture-lifecycle", model="gpt-4.1", premiumModel="gpt-4.1")
    fixture["ui"].update(theme="dark", assistantOpen=False, terminalOpen=False, dashboardCards=dict(weather=False, finance=False, **{"command-center": False}))
    fixture["native"] = dict(activeSection="crew")
    return fixture


def qualify(native_package_directory, output):
    os_release = require_runner(output)
    output.mkdir(parents=True)
    packages = Packages(output)
    manifest = read(native_package_directory / "build.json")
    native_package = native_package_directory / manifest["package"]["file"]
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    if (manifest["sourceDirty"] or manifest["sourceSha"] != head or manifest["profile"] != "release"
            or manifest["target"] != "x86_64-unknown-linux-gnu"
            or native_package.parent != native_package_directory or digest(native_package) != manifest["package"]["sha256"]):
        raise RuntimeError("Expected the committed current-head native deb")
    native_exe = Path("/usr/bin/buddy-native")
    if digest(native_exe) != manifest["binarySha256"]:
        raise RuntimeError("Preserve an existing native installation not owned by this CI build")
    native_name = packages.field(native_package, "Package")
    backend_module = load(HERE / "settings-backend.py")
    github = load(HERE / "github-desktop.py")
    profile = output / "profile"
    profile.mkdir()
    report = dict(sourceSha=head, osRelease=os_release,
        nativeManifest=manifest, cutoverQualified=False, secrets="isolated Secret Service; migration not qualified",
        limitation="Same-head Electron runtime rollback, not an older published-version downgrade")
    write(output / "qualification.json", dict(report, result="incomplete"))
    try:
        with backend_module.SettingsBackend(ROOT) as backend, browser_fixture() as browser_url:
            electron_package = build_electron(packages, backend)
            electron_name = packages.field(electron_package, "Package")
            check = subprocess.run(["dpkg-query", "-W", "-f=${Status}", electron_name], capture_output=True, text=True, timeout=10)
            if "install ok installed" in check.stdout:
                raise RuntimeError("Preserve an existing Electron installation")
            github.mutate(backend, "githubAccounts:create", dict(username="fixture-lifecycle", org="fixture-org", usageProvider="copilot"))
            github.mutate(backend, "bookmarks:create", dict(url="https://example.invalid/lifecycle", title="Lifecycle bookmark", category="Lifecycle"))
            job = github.mutate(backend, "jobs:create", dict(name="Lifecycle inert job", workerType="exec", config=dict(command="exit 0", shell="bash", cwd=str(profile), timeout=1000)))
            github.mutate(backend, "schedules:create", dict(jobId=job, name="Lifecycle paused schedule", cron="0 12 * * *", timezone="UTC", enabled=False, missedPolicy="skip"))
            native_pass(native_exe, output / "fresh-profile", backend, output, "native-fresh", fresh=True)
            # This is the only intentional removal of the preceding job's
            # validated baseline. Earlier failures must leave it untouched.
            packages.remove(native_name, baseline=True)
            packages.install(electron_package)
            listing = subprocess.check_output(["dpkg-query", "-L", electron_name], text=True).splitlines()
            executables = [Path(path) for path in listing if Path(path).name in ("buddy", "Buddy") and Path(path).is_file() and os.access(path, os.X_OK)]
            if len(executables) != 1:
                raise RuntimeError("The installed Electron executable is ambiguous")
            electron_exe = executables[0]
            launchers = {path: digest(Path(path)) for path in listing if path.endswith(".desktop") and Path(path).is_file()}
            if not launchers:
                raise RuntimeError("The Electron package did not register a desktop launcher")
            report["electron"] = dict(packageSha256=digest(electron_package), binarySha256=digest(electron_exe),
                asarSha256=digest(electron_exe.parent / "resources/app.asar"), version=packages.field(electron_package, "Version"),
                desktopLaunchers=launchers)
            fixture = config_fixture()
            write(output / "config-synthetic-input.json", fixture)
            write(profile / "config.json", fixture)
            write(profile / "window-state.json", dict(x=40, y=40, width=1200, height=820, isMaximized=False))
            write(profile / "zoom-level.json", dict(zoomFactor=1.0))
            write(profile / "crew/projects.json", [dict(id=PROJECT, displayName=PROJECT, localPath=str(profile), gitRoot=str(profile), githubSlug="fixture-org/lifecycle", defaultBranch="main", lastOpenedAt=1, lastActiveAt=1)])
            first = electron_pass(electron_exe, profile, backend, output, "electron-seed", browser_url, seed=True)
            seeded = read(profile / "config.json")
            write(output / "config-after-electron-seed.json", seeded)
            seed_baseline = electron_config_baseline(fixture, seeded, layout=first["layout"], phase="electron-seed")
            write(output / "electron-seed-normalization.json", seed_baseline)
            restart = electron_pass(electron_exe, profile, backend, output, "electron-restart", browser_url)
            config_baseline = read(profile / "config.json")
            write(output / "config-before-native.json", config_baseline)
            restart_baseline = electron_config_baseline(seed_baseline["config"], config_baseline,
                layout=restart["layout"], phase="electron-restart")
            write(output / "electron-restart-normalization.json", restart_baseline)
            report["electronBaseline"] = dict(
                seedReceipt="electron-seed-normalization.json", restartReceipt="electron-restart-normalization.json",
                observedRemovals=seed_baseline["observedRemovals"] + restart_baseline["observedRemovals"],
                layoutNormalization=seed_baseline["layoutNormalization"] + restart_baseline["layoutNormalization"],
                unqualifiedBoundary=restart_baseline["unqualifiedBoundary"])
            write(output / "profile-before-native.json", profile_manifest(profile))
            shared = {key: backend.query(key) for key in ("bookmarks:list", "schedules:list", "jobs:list")}
            write(output / "backend-before-native.json", shared)
            baseline = {name: read(profile / name) for name in ("crew/projects.json", "crew/sessions.json", "data-cache.json")}
            packages.install(native_package)
            if any(digest(Path(path)) != sha for path, sha in launchers.items()):
                raise RuntimeError("Native installation changed the existing Electron launcher")
            report["nativeUpgrade"] = native_pass(native_exe, profile, backend, output, "native-upgrade")
            report["nativeRestart"] = native_pass(native_exe, profile, backend, output, "native-restart")
            # Separate selected-section fixtures exercise actual restored rows;
            # they do not pretend the selection itself migrated from Electron.
            for section, actions in (("bookmarks", [("wait_content", "Lifecycle bookmark"), ("capture", "restored-bookmark")]),
                    ("automation", [("content", "Schedules"), ("wait_content", "Lifecycle paused schedule"), ("capture", "restored-schedule")])):
                value = read(profile / "config.json")
                value["native"]["activeSection"] = section
                write(profile / "config.json", value)
                driver = load(HERE / "settings-desktop.py").Driver(native_exe, profile, "x11", backend, ROOT)
                driver.drive("native-" + section, actions)
            value = read(profile / "config.json")
            value["native"]["activeSection"] = "crew"
            write(profile / "config.json", value)
            for name, value in baseline.items():
                require_subset(value, read(profile / name), name)
            after_native = read(profile / "config.json")
            write(output / "config-after-native.json", after_native)
            require_subset(config_baseline, after_native, "qualified pre-native config")
            write(output / "profile-after-native.json", profile_manifest(profile))
            for query, value in shared.items():
                require_subset(value, backend.query(query), query)
            # Genuine package removal/reinstallation, with the same original profile.
            packages.remove(native_name)
            packages.remove(electron_name)
            packages.install(electron_package)
            final = electron_pass(electron_exe, profile, backend, output, "electron-installed-rollback", browser_url)
            after_rollback = read(profile / "config.json")
            write(output / "config-after-rollback.json", after_rollback)
            require_subset(config_baseline, final["config"], "qualified pre-native config through rollback IPC")
            require_subset(config_baseline, after_rollback, "qualified pre-native config after rollback")
            LAYOUT.require_restored(restart["layout"], final["layout"])
            write(output / "profile-after-rollback.json", profile_manifest(profile))
            report.update(result="passed-with-unmet-migration-criteria", restoredElectronData=True,
                unmetCriteria=["Native cannot restore Electron Crew projects/conversations/recovery",
                    "Unknown GitHub account extension preservation is not qualified for the connected Electron baseline",
                    "Chromium-to-WebKit browser profile migration not qualified", "Older published-version downgrade not qualified",
                    "Native cache/session/history consumption not qualified", "Live vault/backend migration not qualified"])
            write(output / "qualification.json", report)
    except BaseException as failure:
        write(output / "qualification.json", dict(report, result="failed", failureType=type(failure).__name__))
        raise
    finally:
        primary = sys.exc_info()[1]
        cleanup_failure = None
        for name in sorted(packages.owned):
            try:
                packages.remove(name)
            except BaseException as failure:
                cleanup_failure = failure
                if primary is not None:
                    primary.add_note(f"Lifecycle package cleanup also failed for {name}")
        if cleanup_failure is not None and primary is None:
            write(output / "qualification.json", dict(report, result="failed", failureType="package cleanup"))
            raise cleanup_failure


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-package-directory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    qualify(args.native_package_directory.resolve(), args.output.resolve())
