"""Installed embedded browser on disposable CI desktops: a page in a tab, restart, refusals.

A loopback fixture site serves the pages. The app admits exactly that origin through
`BUDDY_BROWSER_TEST_ORIGIN` (honored only with an isolated `BUDDY_CONFIG_PATH`) and
appends every refusal to `BUDDY_BROWSER_AUDIT_FILE`. Two bookmarks pointing at the
site are seeded in the same disposable anonymous Convex deployment the Bookmarks
qualification uses. The probe page tries a popup, permission requests, a private
address and a download by itself, so nothing is clicked inside a page.
"""
import argparse
import http.server
import importlib.util
import json
import os
from pathlib import Path
import threading
from urllib.parse import parse_qs, urlparse

from desktop import fixture

HERE = Path(__file__).resolve().parent
CHECKOUT = HERE.parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


terminal = load("terminal-desktop")
github = terminal.github
settings = terminal.settings
LoopbackHTTPServer = load("loopback_http").LoopbackHTTPServer

PAGE_TEXT = "FIXTURE PAGE"
PROBE_TEXT = "PROBE PAGE"
# The probe's last attempt is the download, so its refusal is the notice left showing.
BLOCKED_TEXT = "Downloads are blocked"
THEMES = ("dark", "light")
# Kinds every engine must refuse and record; the others depend on what each engine
# lets an app intercept, and are reported rather than required.
REQUIRED_REFUSALS = {"internal", "download"}

PROBE = """<!doctype html><html><head><title>Buddy Probe</title></head>
<body style="font:40px sans-serif;margin:32px"><p>PROBE PAGE</p><p id="state">running</p>
<iframe src="http://127.0.0.3:PORT/frame" width="240" height="40"></iframe>
<script>
const report = (name, value) => fetch('/beacon?' + new URLSearchParams({name, value}));
let popup = 'error';
try { popup = window.open('/ok', '_blank') ? 'opened' : 'blocked'; } catch (e) { popup = 'threw'; }
report('popup', popup);
if (navigator.geolocation) {
  navigator.geolocation.getCurrentPosition(() => report('geolocation', 'granted'),
    e => report('geolocation', 'denied-' + e.code), {timeout: 3000});
} else { report('geolocation', 'unsupported'); }
if (navigator.mediaDevices && navigator.mediaDevices.getUserMedia) {
  navigator.mediaDevices.getUserMedia({video: true}).then(() => report('camera', 'granted'),
    e => report('camera', 'denied-' + e.name));
} else { report('camera', 'unsupported'); }
if (window.Notification && Notification.requestPermission) {
  Promise.resolve(Notification.requestPermission()).then(p => report('notification', p),
    () => report('notification', 'denied'));
} else { report('notification', 'unsupported'); }
setTimeout(() => { location.href = 'http://127.0.0.2:PORT/private'; }, 1500);
setTimeout(() => { location.href = '/download'; }, 3500);
setTimeout(() => { document.getElementById('state').textContent = 'PROBE DONE'; }, 5000);
</script></body></html>"""


class Site:
    """The fixture pages, the visit cookie, and what the probe reported."""

    def __init__(self):
        self.beacons = {}
        self.downloads = 0
        self.lock = threading.Lock()
        self.server = None

    def page(self, visits):
        return (f"<!doctype html><html><head><title>Buddy Fixture OK</title></head>"
                f"<body style=\"font:48px sans-serif;margin:32px\"><p>{PAGE_TEXT}</p>"
                f"<p>Visit {visits}</p></body></html>")

    def handler(self, site):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def send(self, status, body, content_type="text/html; charset=utf-8", headers=()):
                data = body.encode("utf-8")
                self.send_response(status)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Cache-Control", "no-store")
                for name, value in headers:
                    self.send_header(name, value)
                self.end_headers()
                self.wfile.write(data)

            def do_GET(self):
                url = urlparse(self.path)
                if url.path == "/ok":
                    visits = site.visits(self.headers.get("Cookie", "")) + 1
                    self.send(200, site.page(visits), headers=[
                        ("Set-Cookie", f"visits={visits}; Max-Age=86400; Path=/; SameSite=Lax")])
                elif url.path == "/probe":
                    self.send(200, PROBE.replace("PORT", str(site.server.server_port)))
                elif url.path == "/beacon":
                    query = parse_qs(url.query)
                    with site.lock:
                        site.beacons[query.get("name", [""])[0]] = query.get("value", [""])[0]
                    self.send(204, "")
                elif url.path == "/download":
                    with site.lock:
                        site.downloads += 1
                    self.send(200, "not for saving", "application/octet-stream",
                        [("Content-Disposition", "attachment; filename=fixture.bin")])
                else:
                    self.send(404, "missing")

        return Handler

    @staticmethod
    def visits(cookie):
        for part in cookie.split(";"):
            name, _, value = part.strip().partition("=")
            if name == "visits" and value.isdigit():
                return int(value)
        return 0

    def __enter__(self):
        self.server = LoopbackHTTPServer(("127.0.0.1", 0), self.handler(self))
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return self

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()

    @property
    def origin(self):
        return f"http://127.0.0.1:{self.server.server_port}/"


def bookmarks(origin):
    return [
        dict(url=origin + "probe", title="Probe page", category="Fixture"),
        dict(url=origin + "ok", title="Fixture page", category="Fixture"),
    ]


def require_runner():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Browser desktop qualification requires a disposable GitHub Actions runner")


def seed(backend, origin):
    if backend.query("bookmarks:list") != []:
        raise RuntimeError("The disposable bookmark list is not empty")
    for arguments in bookmarks(origin):
        github.mutate(backend, "bookmarks:create", arguments)


def audit_records(path):
    """(kind, origin) for each refusal the app recorded."""
    if not path.exists():
        return []
    records = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]
    for record in records:
        # Only the origin may be recorded, never a path or query.
        if set(record) != {"kind", "origin", "reason"} or "/" in record["origin"].removeprefix("http://").removeprefix("https://"):
            raise RuntimeError(f"Unexpected audit record {record}")
    return [(record["kind"], record["origin"]) for record in records]


def frame_origin(port):
    """The private address only the probe's iframe uses."""
    return f"http://127.0.0.3:{port}"


def evaluate(records, beacons, downloads, frame):
    """The refusals every engine must make, and nothing the probe asked for granted."""
    kinds = [kind for kind, _ in records]
    missing = REQUIRED_REFUSALS - set(kinds)
    if missing:
        raise RuntimeError(f"Expected refusals were not recorded: {sorted(missing)} (recorded {kinds})")
    # A frame reaches the network separately from the page (WebView2 reports it
    # through its own event), so its refusal is checked on its own.
    if ("internal", frame) not in records:
        raise RuntimeError(f"The probe's iframe to {frame} was not refused (recorded {records})")
    if beacons.get("popup") not in ("blocked", "threw"):
        raise RuntimeError(f"The probe's popup was not refused: {beacons.get('popup')}")
    granted = sorted(name for name, value in beacons.items() if value == "granted")
    if granted:
        raise RuntimeError(f"The probe was granted {granted}")
    if downloads < 1:
        raise RuntimeError("The probe never requested its download")
    return dict(refusals=sorted(set(kinds)), beacons=beacons)


def first_pass(driver, theme):
    """A fresh session: the probe's refusals, then a page, at both sizes."""
    flow = driver.drive(f"browser-{theme}", [
        ("wait_text", "Probe page"), ("click", "Probe page"),
        ("wait_text", BLOCKED_TEXT), ("capture", f"blocked-{theme}"),
        ("click", "Bookmarks"), ("wait_text", "Fixture page"), ("capture", f"tabs-list-{theme}"),
        ("click", "Fixture page"), ("wait_text", "Visit 1"), ("capture", f"page-{theme}"),
        ("resize", 900, 650), ("sleep", 1), ("capture", f"page-compact-{theme}"),
        ("resize", 1200, 820),
    ], record=True)
    github.check(flow / f"blocked-{theme}.png", PROBE_TEXT, BLOCKED_TEXT, "Bookmarks", "Buddy Probe")
    github.check(flow / f"tabs-list-{theme}.png", "Fixture page", "Probe page", "Buddy Probe")
    # The page's own text and its tab title prove which page is shown. The
    # address field's dim text read in no OCR layout on Windows (run 40647223188).
    github.check(flow / f"page-{theme}.png", PAGE_TEXT, "Visit 1", "Buddy Fixture OK")
    github.check(flow / f"page-compact-{theme}.png", PAGE_TEXT, "Buddy Fixture OK")


def restart_pass(driver, theme, visit):
    """The saved tabs come back, and the page's cookie survived the restart."""
    flow = driver.drive(f"restart-{theme}", [
        ("wait_text", f"Visit {visit}"), ("capture", f"restored-{theme}"),
    ], record=False)
    github.check(flow / f"restored-{theme}.png", PAGE_TEXT, f"Visit {visit}", "Buddy Probe", "Buddy Fixture OK")


def light_pass(driver, visit):
    flow = driver.drive("browser-light", [
        ("wait_text", f"Visit {visit}"), ("capture", "page-light"),
        ("resize", 900, 650), ("sleep", 1), ("capture", "page-compact-light"),
        ("resize", 1200, 820),
        ("click", "Buddy Probe"), ("wait_text", BLOCKED_TEXT), ("capture", "blocked-light"),
    ], record=True)
    github.check(flow / "page-light.png", PAGE_TEXT, f"Visit {visit}", "Buddy Fixture OK")
    github.check(flow / "page-compact-light.png", PAGE_TEXT)
    github.check(flow / "blocked-light.png", PROBE_TEXT, BLOCKED_TEXT)


def shortcut_pass(driver):
    """X11 sends a key to the window under the pointer. With the pointer resting
    over a page, Buddy's shortcuts still act on Buddy (#880): the tab shortcuts
    here, and Ctrl+Q, which ends the drive."""
    flow = driver.drive("shortcuts-light", [
        ("wait_text", PROBE_TEXT),
        # Clicking a tab gives the browser the keyboard.
        ("click", "Buddy Probe"), ("hover",),
        ("shortcut", "Tab"), ("wait_text", PAGE_TEXT), ("capture", "next-tab-light"),
        ("shortcut", "shift+Tab"), ("wait_text", PROBE_TEXT),
        ("shortcut", "F4"), ("wait_text", PAGE_TEXT), ("capture", "closed-tab-light"),
    ], record=False)
    github.check(flow / "next-tab-light.png", PAGE_TEXT, "Buddy Probe", "Buddy Fixture OK")
    github.check(flow / "closed-tab-light.png", PAGE_TEXT, "Buddy Fixture OK")


def saved_tabs(config):
    return [tab["url"] for tab in json.loads(config.read_text(encoding="utf-8"))["native"].get("browser", {}).get("tabs", [])]


def qualify(executable, output, session, version, backend, site):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")
    terminal.write_config(config, lambda value: value["native"].update(activeSection="bookmarks"))
    audit = output / "browser-audit.jsonl"
    os.environ["BUDDY_BROWSER_TEST_ORIGIN"] = site.origin
    os.environ["BUDDY_BROWSER_AUDIT_FILE"] = str(audit)
    seed(backend, site.origin)
    driver = settings.Driver(executable, output, session, backend, CHECKOUT)
    # WebView2 can't create pages on the driver's hidden Windows desktop.
    driver.input_desktop = True
    first_pass(driver, "dark")
    restart_pass(driver, "dark", 2)
    light = json.loads(fixture(config.parent / "light-template", "light").read_text())
    terminal.write_config(config, lambda value: value["ui"].update(light["ui"]))
    light_pass(driver, 3)
    with site.lock:
        result = evaluate(audit_records(audit), dict(site.beacons), site.downloads,
            frame_origin(site.server.server_port))
    pages = [item["url"] for item in bookmarks(site.origin)]
    if saved_tabs(config) != pages:
        raise RuntimeError(f"Unexpected saved tabs: {saved_tabs(config)}")
    checks = []
    if session == "x11":
        shortcut_pass(driver)
        # Ctrl+F4 closed the probe's tab.
        if saved_tabs(config) != pages[1:]:
            raise RuntimeError(f"Ctrl+F4 over a page did not close its tab: {saved_tabs(config)}")
        checks = ["Buddy's shortcuts with the pointer over a page"]
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True, engine=dict(windows="WebView2", macos="WKWebView",
        x11="WebKitGTK")[session], **result,
        dataSource="Installed app; loopback fixture site admitted by exact origin; bookmarks in an isolated anonymous Convex deployment",
        checks=["bookmark opens in a tab", "page renders inside the window", "private address refused",
            "download refused", "private iframe refused", "popup refused", "no permission granted", "tabs and cookies survive restart",
            "compact window", "light/dark", *checks]), indent=2), encoding="utf-8")


def main():
    # Check the host before argparse, filesystem access, or any input.
    require_runner()
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    output = terminal.owned_output(args.output)
    executable = args.executable.resolve(strict=True)
    with settings.SettingsBackend(CHECKOUT) as backend, Site() as site:
        qualify(executable, output, args.session, args.version, backend, site)


if __name__ == "__main__":
    main()
