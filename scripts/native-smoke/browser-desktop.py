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
import struct
import sys
import threading
import time
from urllib.parse import parse_qs, urlparse
import zlib

import colour_blocks
from desktop import fixture, require_closed_sessions

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
# The Fixture page bookmark's favicon: one solid colour no theme or globe icon uses.
FAVICON_RGB = (255, 0, 255)
# A 20 px card icon is 400 pixels at 100%; corners and scaling take a few.
FAVICON_MIN_PIXELS = 150
# A 12 px tree row icon is 144 pixels at 100%; the same share of it.
TREE_FAVICON_MIN_PIXELS = 50
# The toolbar's page zoom before and after one Alt+= (the 10% button step).
ZOOM_BEFORE, ZOOM_AFTER = "100%", "110%"
# A lime block of fixed CSS size on the Fixture page: one Alt+= must widen it
# by the step, so a zoom the engine refused fails even if the toolbar moved.
PAGE_BLOCK = "lime"
PAGE_BLOCK_CSS = "width:160px;height:80px;background:#00ff00"
ZOOM_RATIO = (1.05, 1.15)
# After Alt+- the block is back at its 100% size, give or take a pixel of rounding;
# a page left at 110% is 10% off.
UNZOOM_RATIO = (0.97, 1.03)


def solid_png(width, height, rgb):
    """An opaque single-colour PNG, built without an imaging library."""
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(rgb) * width for _ in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


FAVICON = solid_png(32, 32, FAVICON_RGB)

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
        self.favicons = 0
        self.lock = threading.Lock()
        self.server = None

    def page(self, visits):
        return (f"<!doctype html><html><head><title>Buddy Fixture OK</title></head>"
                f"<body style=\"font:48px sans-serif;margin:32px\"><p>{PAGE_TEXT}</p>"
                f"<p>Visit {visits}</p><div style=\"{PAGE_BLOCK_CSS}\"></div></body></html>")

    def handler(self, site):
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def send(self, status, body, content_type="text/html; charset=utf-8", headers=()):
                data = body if isinstance(body, bytes) else body.encode("utf-8")
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
                elif url.path == "/favicon.png":
                    with site.lock:
                        site.favicons += 1
                    self.send(200, FAVICON, "image/png")
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
        dict(url=origin + "ok", title="Fixture page", category="Fixture", faviconUrl=origin + "favicon.png"),
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


def favicon_pixels(image):
    """How many pixels of the capture are the fixture favicon's colour, and their bounds."""
    return colour_blocks.measure(image, "magenta")


def require_favicon(image):
    """The Fixture page card shows the served favicon, not the globe: a block of
    its colour about icon-sized, which nothing else in Buddy paints."""
    count, bounds = favicon_pixels(image)
    image.with_suffix(".favicon.json").write_text(json.dumps(dict(pixels=count, bounds=bounds)), encoding="utf-8")
    if count < FAVICON_MIN_PIXELS:
        raise RuntimeError(f"The bookmark card shows no favicon ({count} favicon-coloured pixels)")
    # Two icons at most (the card and an expanded tree row), each under 64 px a side.
    if bounds["width"] > 160 or bounds["height"] > 160:
        raise RuntimeError(f"The favicon colour covers more than an icon: {bounds}")
    return dict(pixels=count, bounds=bounds)


def require_tree_favicon(image):
    """The expanded tree's Fixture page row shows the favicon too: two separate
    magenta blocks, and the left one, in the tree pane beside the list, is the
    row's icon, smaller than the card's. A tree row that lost its favicon
    leaves only the card's block, so the card can't pass for it."""
    blocks = colour_blocks.large_blocks(image, "magenta", TREE_FAVICON_MIN_PIXELS)
    image.with_suffix(".favicon.json").write_text(json.dumps(dict(blocks=blocks)), encoding="utf-8")
    if len(blocks) != 2:
        raise RuntimeError(f"The bookmark tree row and card do not both show the favicon ({len(blocks)} magenta blocks)")
    tree, card = blocks
    if any(block["width"] > 64 or block["height"] > 64 for block in blocks):
        raise RuntimeError(f"The favicon colour covers more than an icon: {blocks}")
    if tree["width"] >= card["width"] or tree["height"] >= card["height"]:
        raise RuntimeError(f"The block left of the card is not the tree row's smaller icon: {blocks}")
    return dict(tree=tree, card=card)


def require_scaled(before, after, expected, failure):
    """The page's fixed-size block in `after`, against `before`, is within the
    `expected` width and height ratios; the measurements are saved beside `after`."""
    sizes = []
    for image in (before, after):
        count, bounds = colour_blocks.measure(image, PAGE_BLOCK)
        if bounds is None or count < 1000:
            raise RuntimeError(f"The Fixture page's block is not shown in {image.name} ({count} pixels)")
        sizes.append(bounds)
    ratios = [sizes[1][side] / sizes[0][side] for side in ("width", "height")]
    evidence = dict(before=sizes[0], after=sizes[1], ratios=ratios)
    after.with_suffix(".zoom.json").write_text(json.dumps(evidence), encoding="utf-8")
    if not all(expected[0] <= ratio <= expected[1] for ratio in ratios):
        raise RuntimeError(f"{failure}: {evidence}")
    return evidence


def require_zoomed(before, after):
    """The page's content grew by one zoom step between the two captures: its
    fixed-size block is wider and taller by about 10%, not just relabelled."""
    return require_scaled(before, after, ZOOM_RATIO, "The page did not scale by one zoom step")


def require_unzoomed(before, after):
    """After Alt+- the page's content is back at its 100% size, not just relabelled."""
    return require_scaled(before, after, UNZOOM_RATIO, "The page did not return to its 100% size")


# The all-bookmarks tab. Its own word "All" is the fallback: on macOS
# (run 45719379502) Vision read the light tab as "Bookman: All".
BOOKMARKS_TAB = "Bookmarks: All|All"


# The tree's Fixture page row. Vision on Intel read its magenta favicon as a
# letter glued to the title, "JFixture page" (run 45737486513).
FIXTURE_ROW = "Fixture page|JFixture page"
# The probe's tab. Its own word "Probe" is unique in the tab row; on macOS
# Vision read the light tab as "Buy Probe" (run 45737486513).
PROBE_TAB = "Buddy Probe|Probe"


def favicon_steps(theme):
    """The bookmark list, then its favicon: it arrives after the list, so its
    colour is waited for before the capture."""
    return [
        ("workspace", BOOKMARKS_TAB), ("wait_text", "Fixture page"),
        ("wait_colour", "magenta", FAVICON_MIN_PIXELS), ("capture", f"bookmark-favicons-{theme}"),
    ]


def zoom_steps(theme):
    """Alt+= on the shown page, then Alt+- back to 100%. The engine repaints
    the page a frame after it takes the zoom."""
    return [
        ("altkey", "="), ("wait_text", ZOOM_AFTER), ("sleep", 1), ("capture", f"browser-page-zoom-{theme}"),
        ("altkey", "-"), ("wait_text", ZOOM_BEFORE), ("sleep", 1), ("capture", f"browser-page-unzoom-{theme}"),
    ]


def tree_favicon_steps(theme):
    """Choose the Fixture category in the bookmark tree, which expands it, so
    its Fixture page row shows the favicon beside the card's. Each card also
    shows "Fixture" (its category), so the click is bounded by the toolbar's
    Refresh above and the Search label below, where only the tree's row is.
    It opens the "Bookmarks: Fixture" tab, counted like an Electron
    `openTab('bookmarks-category:Fixture')`."""
    return [
        ("workspace", BOOKMARKS_TAB), ("wait_text", "Fixture page"),
        ("between", "Fixture", "Refresh", "Search"),
        ("wait_colour", "magenta", TREE_FAVICON_MIN_PIXELS, 2), ("capture", f"bookmark-tree-favicons-{theme}"),
    ]


def require_tree_favicons(flow, theme):
    """The tree capture shows the chosen category, and its row's favicon."""
    github.check(flow / f"bookmark-tree-favicons-{theme}.png", "In Fixture", "Fixture page", "Probe page", browser=True)
    return require_tree_favicon(flow / f"bookmark-tree-favicons-{theme}.png")


def require_toolbar_label(path, label):
    """The toolbar's small zoom label on a capture. The whole-capture OCR of
    `github.check` can miss it on Windows even though the drive's live
    `wait_text` found it (run 45737486513), so the label locator that wait
    uses is tried on the same capture too. Either reading suffices; neither
    keeps the check's failure."""
    try:
        github.check(path, label, browser=True)
        return
    except RuntimeError as failure:
        missing = failure
    engines = [{}] + ([dict(native_macos=True)] if sys.platform == "darwin" else [])
    for options in engines:
        try:
            settings.ocr.locate_image(path, label, **options)
            return
        except settings.ocr.AmbiguousLabel:
            return
        except RuntimeError:
            pass
    raise missing


def require_favicon_and_zoom(flow, theme):
    """The favicon capture shows the icon, and the zoom captures show the page
    one step larger and then back at its size in `page-{theme}`."""
    github.check(flow / f"bookmark-favicons-{theme}.png", "Fixture page", "Probe page", browser=True)
    favicon = require_favicon(flow / f"bookmark-favicons-{theme}.png")
    # The large page heading and the small toolbar label need different OCR
    # preprocessing, so one layout rarely reads both (Windows, run
    # 45719379502); each is required on the same capture separately.
    for name, label in ((f"browser-page-zoom-{theme}.png", ZOOM_AFTER), (f"browser-page-unzoom-{theme}.png", ZOOM_BEFORE)):
        github.check(flow / name, PAGE_TEXT, browser=True)
        require_toolbar_label(flow / name, label)
    zoom = require_zoomed(flow / f"page-{theme}.png", flow / f"browser-page-zoom-{theme}.png")
    unzoom = require_unzoomed(flow / f"page-{theme}.png", flow / f"browser-page-unzoom-{theme}.png")
    return dict(favicon=favicon, zoom=zoom, unzoom=unzoom)


def first_pass(driver, theme):
    """A fresh session: the probe's refusals, then a page, at both sizes. The
    Fixture page card shows its favicon, Alt+= zooms the page one step, and
    Alt+- takes it back to its 100% size (so later passes see the saved zoom
    unchanged). Then the Fixture category is expanded in the tree, whose row
    shows the favicon too, and that row opens the page again, where the drive
    ends for the restart to restore."""
    flow = driver.drive(f"browser-{theme}", [
        ("wait_text", "Probe page"), ("content", "Probe page"),
        ("wait_text", BLOCKED_TEXT), ("capture", f"blocked-{theme}"),
        ("workspace", "Bookmarks: All"), ("wait_text", "Fixture page"), ("capture", f"tabs-list-{theme}"),
        *favicon_steps(theme)[2:],
        ("content", "Fixture page"), ("wait_text", "Visit 1"), ("capture", f"page-{theme}"),
        *zoom_steps(theme),
        ("resize", 900, 650), ("sleep", 1), ("capture", f"page-compact-{theme}"),
        ("resize", 1200, 820),
        *tree_favicon_steps(theme),
        # Repeat after the title-qualified captures, from the tree row this
        # time: reusing a URL selects its bookmark title, and it counts without
        # a new HTTP visit. The search leaves only the Probe page card, so the
        # row is the one "Fixture page" shown.
        ("box", "Search", "Probe"), ("sleep", 1), ("content", FIXTURE_ROW),
        ("wait_text", "Visit 1"),
    ], record=True)
    github.check(flow / f"blocked-{theme}.png", PROBE_TEXT, BLOCKED_TEXT, "Bookmarks", "Buddy Probe", browser=True)
    github.check(flow / f"tabs-list-{theme}.png", "Fixture page", "Probe page", "Buddy Probe", browser=True)
    # The page's own text and its tab title prove which page is shown. The
    # address field's dim text read in no OCR layout on Windows (run 40647223188).
    github.check(flow / f"page-{theme}.png", PAGE_TEXT, "Visit 1", "Buddy Fixture OK", browser=True)
    github.check(flow / f"page-compact-{theme}.png", PAGE_TEXT, "Buddy Fixture OK", browser=True)
    return dict(**require_favicon_and_zoom(flow, theme), tree_favicon=require_tree_favicons(flow, theme))


def saved_zoom(config):
    """The page zoom saved in `native.browser`, or None when none is saved."""
    return json.loads(config.read_text(encoding="utf-8-sig"))["native"].get("browser", {}).get("zoom")


def restore_zoom(config, zoom):
    def change(value):
        browser = value["native"].get("browser")
        if browser is None:
            return
        if zoom is None:
            browser.pop("zoom", None)
        else:
            browser["zoom"] = zoom
    terminal.write_config(config, change)


def restart_pass(driver, theme, visit):
    """The saved tabs come back, and the page's cookie survived the restart."""
    flow = driver.drive(f"restart-{theme}", [
        ("wait_text", f"Visit {visit}"), ("capture", f"restored-{theme}"),
    ], record=False)
    github.check(flow / f"restored-{theme}.png", PAGE_TEXT, f"Visit {visit}", "Buddy Probe", "Buddy Fixture OK", browser=True)


def light_pass(driver, visit):
    """The restored session in the light theme: the page at both sizes, the
    Fixture page card's favicon, then that card is opened again (selecting its
    tab, without a new visit, and leaving the keyboard in the browser) and
    Alt+= and Alt+- zoom the page and take it back. The Fixture category is
    then expanded in the tree, whose row shows the favicon too. The drive is
    recorded, so the keyboard zoom flow is in its `interaction.mp4`. It ends
    on the probe's tab, where the X11 shortcut pass starts."""
    flow = driver.drive("browser-light", [
        ("wait_text", f"Visit {visit}"), ("capture", "page-light"),
        ("resize", 900, 650), ("sleep", 1), ("capture", "page-compact-light"),
        ("resize", 1200, 820),
        *favicon_steps("light"),
        # The fourth explicit bookmark open, counted like the others.
        ("content", "Fixture page"), ("wait_text", f"Visit {visit}"),
        *zoom_steps("light"),
        *tree_favicon_steps("light"),
        ("workspace", PROBE_TAB), ("wait_text", BLOCKED_TEXT), ("capture", "blocked-light"),
    ], record=True)
    github.check(flow / "page-light.png", PAGE_TEXT, f"Visit {visit}", "Buddy Fixture OK", browser=True)
    github.check(flow / "page-compact-light.png", PAGE_TEXT, browser=True)
    github.check(flow / "blocked-light.png", PROBE_TEXT, BLOCKED_TEXT, browser=True)
    return dict(**require_favicon_and_zoom(flow, "light"), tree_favicon=require_tree_favicons(flow, "light"))


class BrowserDriver(settings.Driver):
    def interact(self, window, directory, kind, values):
        if kind != "tooltip_leave":
            return super().interact(window, directory, kind, values)
        # Locate the tab while the pointer is on the page, then deliberately
        # show its tooltip. OCR must see both copies before we test the leave.
        image = directory / "tooltip-target.png"
        label = self.locate(window, image, "workspace", "Buddy Probe")
        settings.run("xdotool", "mousemove", "--window", window.window,
            int(label["left"] + label["width"] / 2), int(label["top"] + label["height"] / 2))
        time.sleep(2)
        before = directory / "tooltip-on-tab-light.png"
        window.screenshot(before)
        try:
            settings.ocr.locate_image(before, "Buddy Probe")
        except settings.ocr.AmbiguousLabel:
            pass
        else:
            raise RuntimeError("The tab tooltip did not appear before testing page entry")
        window.hover("page")
        time.sleep(1)
        after = directory / "tooltip-over-page-light.png"
        window.screenshot(after)
        # A stale tooltip leaves two visible titles and fails this lookup.
        settings.ocr.locate_image(after, "Buddy Probe")


def shortcut_pass(driver):
    """X11 sends a key to the window under the pointer. With the pointer resting
    over a page, Buddy's shortcuts still act on Buddy (#880): the tab shortcuts
    here, and Ctrl+Q, which ends the drive."""
    flow = driver.drive("shortcuts-light", [
        ("hover", "page"), ("wait_text", PROBE_TEXT),
        ("tooltip_leave",),
        # Clicking a tab gives the browser the keyboard.
        ("workspace", "Buddy Probe"), ("hover", "page"),
        ("shortcut", "Tab"), ("wait_text", PAGE_TEXT), ("capture", "next-tab-light"),
        ("shortcut", "shift+Tab"), ("wait_text", PROBE_TEXT),
        ("shortcut", "F4"), ("wait_text", PAGE_TEXT), ("capture", "closed-tab-light"),
    ], record=True)
    github.check(flow / "next-tab-light.png", PAGE_TEXT, "Buddy Probe", "Buddy Fixture OK", browser=True)
    github.check(flow / "closed-tab-light.png", PAGE_TEXT, "Buddy Fixture OK", browser=True)


def saved_tabs(config):
    return [tab["url"] for tab in json.loads(config.read_text(encoding="utf-8"))["native"].get("browser", {}).get("tabs", [])]


def require_bookmark_activity(backend, before, opens, launches):
    """`opens` embedded bookmark and category opens, plus the Dashboard each launch opens,
    as Electron's tab list does when its window starts, and one closed
    session per launch."""
    after = backend.query("buddyStats:get")
    expected = dict(tabsOpened=opens + launches, prsViewed=0, prsReviewed=0, prsMergedWatched=0)
    observed = {field: after.get(field, 0) - before.get(field, 0) for field in expected}
    if observed != expected:
        raise RuntimeError(f"Bookmark tab activity differs from explicit opens: {observed}, expected {expected}")
    require_closed_sessions(before, after, launches)
    return after


def qualify(executable, output, session, version, backend, site):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")
    terminal.write_config(config, lambda value: value["native"].update(activeSection="bookmarks"))
    audit = output / "browser-audit.jsonl"
    os.environ["BUDDY_BROWSER_TEST_ORIGIN"] = site.origin
    os.environ["BUDDY_BROWSER_AUDIT_FILE"] = str(audit)
    seed(backend, site.origin)
    driver = BrowserDriver(executable, output, session, backend, CHECKOUT)
    # WebView2 can't create pages on the driver's hidden Windows desktop.
    driver.input_desktop = True
    activity_before = backend.query("buddyStats:get")
    zoom = saved_zoom(config)
    try:
        first = first_pass(driver, "dark")
    finally:
        # Alt+- already took the page back; a failed drive may not have.
        restore_zoom(config, zoom)
    with site.lock:
        if site.favicons < 1:
            raise RuntimeError("Buddy never asked the fixture site for the bookmark's favicon")
        first["favicon"]["requests"] = site.favicons
    # Three bookmark opens (the last from the tree) and the Fixture category
    # chosen in the tree.
    activity_opened = require_bookmark_activity(backend, activity_before, 4, 1)
    restart_pass(driver, "dark", 2)
    activity_restored = require_bookmark_activity(backend, activity_before, 4, 2)
    template = json.loads(fixture(config.parent / "light-template", "light").read_text())
    terminal.write_config(config, lambda value: value["ui"].update(template["ui"]))
    zoom = saved_zoom(config)
    try:
        light = light_pass(driver, 3)
    finally:
        restore_zoom(config, zoom)
    # The Fixture page card opened again, and the category chosen again.
    activity_switched = require_bookmark_activity(backend, activity_before, 6, 3)
    (output / "bookmark-activity.json").write_text(json.dumps(dict(
        before=activity_before, opened=activity_opened, restored=activity_restored,
        switched=activity_switched, dataSource="isolated anonymous Convex fixture",
        checks=["four explicit bookmark opens including two of a repeated URL",
            "two category opens from the tree", "restart persistence",
            "tab restoration and switching do not write", "PR counters unchanged",
            "one Dashboard open and one closed session per launch"]), indent=2), encoding="utf-8")
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
        x11="WebKitGTK")[session], **result, **first, light=light,
        dataSource="Installed app; loopback fixture site admitted by exact origin; bookmarks in an isolated anonymous Convex deployment",
        checks=["bookmark opens in a tab", "page renders inside the window", "bookmark card favicon in both themes",
            "bookmark tree row favicon in both themes",
            "Alt+= and Alt+- zoom the page in both themes", "private address refused",
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
