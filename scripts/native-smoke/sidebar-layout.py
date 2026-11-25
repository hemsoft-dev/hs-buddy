"""Qualify the installed sidebar split from ui.paneSizes and the root zoom from ui.zoomLevel.

The GitHub section shows the shell sidebar. Each capture is measured from its
own pixels: the activity bar's edge and the 1px divider line are the first two
vertical edges in the empty rows above the status footer, so the sidebar's
share of the row needs no platform scale, window frame or DPI.
"""
import json
import math
import struct

# Saved Electron sizes that differ from the [300, 900] default: a 40% share.
SAVED_SIZES = [480, 720]
SAVED_SHARE = 0.4
# Where the divider drag ends, as a share of the row; the saved file must follow.
DRAG_SHARE = 0.25
DEFAULT_SHARE = 0.25
# A valid ui.zoomLevel (Electron's 80-150), and the scale it gives the shell.
ZOOM_LEVEL = 125
# A divider within 3% of the row of where it belongs (about 35px at 1,152px).
SHARE_SLACK = 0.03
ZOOM_SLACK = 0.06
# A colour step any channel makes between neighbouring pixels at an edge. The
# dark divider is 30 against 60 and the light one 229 against 255.
EDGE_STEP = 12
# Empty rows under the sidebar list and the PR cards, above the footer.
SAMPLE_ROWS = (0.84, 0.87, 0.90)
# Edges closer than this belong to one painted line (a 2px HiDPI divider).
LINE_GAP = 3
# Windows captures include the window's invisible resize border: 8px of pure
# black at the left, right and bottom, which the dark theme never paints. A
# frame is never wider than 2% of the row.
FRAME = (0, 0, 0)


def capture_size(image):
    return struct.unpack(">II", image.read_bytes()[16:24])


def row_pixels(image, y, width, run):
    """One untouched capture row as RGB triples, read through ffmpeg."""
    raw = image.with_suffix(f".row-{y}.rgb")
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", image,
        "-vf", f"crop={width}:1:0:{y}:exact=1,format=rgb24", "-f", "rawvideo", "-frames:v", "1", raw)
    data = raw.read_bytes()
    if len(data) != width * 3:
        raise RuntimeError("The split analysis read an incomplete capture row")
    return [tuple(data[index * 3:index * 3 + 3]) for index in range(width)]


def edges(pixels):
    """Columns where the colour steps from the column before them."""
    return [x for x in range(1, len(pixels))
        if max(abs(a - b) for a, b in zip(pixels[x - 1], pixels[x])) > EDGE_STEP]


def lines(columns):
    """Group neighbouring edge columns into painted features: (first, last)."""
    groups = []
    for x in columns:
        if groups and x - groups[-1][1] <= LINE_GAP:
            groups[-1][1] = x
        else:
            groups.append([x, x])
    return [tuple(group) for group in groups]


def thin_lines(pixels):
    """The painted lines in one row: edge groups no wider than a 2px HiDPI line.

    A window border (inside the first 2% of the row; the 48px bar is wider
    than that in any window) and the activity bar's icons, whose glyphs
    paint blocks wider than a line, are left out.
    """
    width = len(pixels)
    return [group for group in lines(edges(pixels))
        if group[0] >= 0.02 * width and group[1] - group[0] <= LINE_GAP]


def centre(group):
    return (group[0] + group[1]) / 2


def row_split(pixels):
    """The activity bar's edge and the divider's centre in one row."""
    features = thin_lines(pixels)
    if len(features) < 2:
        raise RuntimeError("The capture row shows no activity bar edge and sidebar divider")
    bar, divider = features[0], features[1]
    return bar[0], centre(divider)


def window_columns(rows, width):
    """The first column the window paints and the column after its last,
    leaving out a capture frame that every sampled row shows."""
    limit = int(0.02 * width)

    def framed(x):
        return all(pixels[x] == FRAME for pixels in rows)

    left, right = 0, width
    while left < limit and framed(left):
        left += 1
    while width - right < limit and framed(right - 1):
        right -= 1
    return left, right


def split_geometry(rows, width):
    """The activity bar's edge and the divider: the first two lines that every
    sampled row paints in the same place, and the sidebar's share.

    A sampled row can cross an activity bar icon (the bar's icons move with
    ui.zoomLevel), whose glyph strokes are edges in that row alone; the bar's
    edge and the divider run the window's full height, so every row has them.
    """
    measured = [thin_lines(pixels) for pixels in rows]
    if any(len(features) < 2 for features in measured):
        raise RuntimeError("The capture row shows no activity bar edge and sidebar divider")
    common = [group for group in measured[0]
        if all(any(abs(centre(group) - centre(other)) <= 1 for other in features) for features in measured[1:])]
    if len(common) < 2:
        raise RuntimeError("The sampled rows disagree on the sidebar divider")
    bar, divider = common[0][0], centre(common[1])
    left, right = window_columns(rows, width)
    if not left < bar < divider < right:
        raise RuntimeError("The measured sidebar divider is outside the row")
    return dict(width=width, left=left, right=right, bar=bar, divider=divider,
        share=(divider - bar) / (right - bar))


def window(geometry):
    """The window's painted columns in a capture: (left, right)."""
    return geometry.get("left", 0), geometry.get("right", geometry["width"])


def bar_width(geometry):
    """The activity bar as painted, without any capture frame before it."""
    return geometry["bar"] - window(geometry)[0]


def measure(image, run):
    """Measure one capture and keep the receipt beside it."""
    width, height = capture_size(image)
    rows = [row_pixels(image, int(height * fraction), width, run) for fraction in SAMPLE_ROWS]
    geometry = split_geometry(rows, width)
    geometry["rows"] = [int(height * fraction) for fraction in SAMPLE_ROWS]
    image.with_suffix(".split.json").write_text(json.dumps(geometry), encoding="utf-8")
    return geometry


# pane_sizes.rs at 100%: the activity bar, and the sidebar and content
# minimums that hold a saved share (assistant closed).
ACTIVITY_BAR = 48
SIDEBAR_MIN = 200
CONTENT_MIN = 400


def expected_share(share, geometry):
    """The share the native layout gives a saved `share` in this capture.

    The activity bar is 48px at 100%, so it gives the capture's pixels per
    100% pixel, whatever the platform scale and ui.zoomLevel; the row in
    100% pixels then decides whether a minimum holds the sidebar.
    """
    unit = bar_width(geometry) / ACTIVITY_BAR
    row = (window(geometry)[1] - geometry["bar"]) / unit
    if row < SIDEBAR_MIN + CONTENT_MIN:
        return SIDEBAR_MIN / (SIDEBAR_MIN + CONTENT_MIN)
    return min(max(share * row, SIDEBAR_MIN), row - CONTENT_MIN) / row


def require_share(geometry, share, what):
    expected = expected_share(share, geometry)
    if not math.isclose(geometry["share"], expected, abs_tol=SHARE_SLACK):
        raise RuntimeError(f"The {what} sidebar takes {geometry['share']:.3f} of the row, expected {expected:.3f}")


def saved_share(sizes):
    """The sidebar share a saved ui.paneSizes gives with the assistant closed."""
    if (not isinstance(sizes, list) or len(sizes) < 2 or any(isinstance(size, bool)
            or not isinstance(size, (int, float)) or not math.isfinite(size) or size <= 0 for size in sizes)):
        raise RuntimeError("The saved ui.paneSizes is not a list of positive widths")
    return sizes[0] / (sizes[0] + sizes[1])


def drag_points(geometry, share):
    """Capture-pixel start, row and end of a divider drag to `share` of the row."""
    start = round(geometry["divider"])
    end = round(geometry["bar"] + share * (window(geometry)[1] - geometry["bar"]))
    if abs(end - start) < 20:
        raise RuntimeError("The planned divider drag is too short to prove a save")
    return start, geometry["rows"][1], end


def edit_ui(config, **changes):
    value = json.loads(config.read_text(encoding="utf-8-sig"))
    ui = value.setdefault("ui", {})
    for key, change in changes.items():
        if change is None:
            ui.pop(key, None)
        else:
            ui[key] = change
    value.setdefault("native", {})["activeSection"] = "github"
    config.write_text(json.dumps(value), encoding="utf-8")


def qualify(driver, config, check, run):
    """Launch at saved sizes, drag the divider, then launch zoomed.

    `check` proves visible text in one OCR layout (github-desktop.check).
    The fixture's own ui.paneSizes and ui.zoomLevel are restored afterwards.
    """
    original = json.loads(config.read_text(encoding="utf-8-sig")).get("ui", {})
    keep = {key: original.get(key) for key in ("paneSizes", "zoomLevel", "assistantOpen")}
    try:
        edit_ui(config, paneSizes=SAVED_SIZES, zoomLevel=100, assistantOpen=False)
        saved = driver.drive("sidebar-width-dark", [("sleep", 2), ("capture", "sidebar-width-dark")])
        width = saved / "sidebar-width-dark.png"
        check(width, "My PRs", "Repositories")
        launched = measure(width, run)
        require_share(launched, SAVED_SHARE, "launched")

        start, row, end = drag_points(launched, DRAG_SHARE)
        dragged = driver.drive("sidebar-drag-dark", [("sleep", 2), ("capture", "sidebar-drag-before-dark"),
            ("drag", start, row, end), ("sleep", 2), ("capture", "sidebar-drag-dark")], record=True)
        before = measure(dragged / "sidebar-drag-before-dark.png", run)
        if abs(before["divider"] - launched["divider"]) > 2 or before["width"] != launched["width"]:
            raise RuntimeError("A second launch from the same file laid out a different sidebar")
        after = measure(dragged / "sidebar-drag-dark.png", run)
        require_share(after, DRAG_SHARE, "dragged")
        sizes = json.loads(config.read_text(encoding="utf-8-sig"))["ui"].get("paneSizes")
        # The file holds the share the divider shows, which the sidebar's
        # minimum may hold above the drag's target (a narrow row at a high
        # DPI), so it is held to the same clamp as the capture.
        wanted = expected_share(DRAG_SHARE, after)
        if not math.isclose(saved_share(sizes), wanted, abs_tol=SHARE_SLACK):
            raise RuntimeError(f"The divider drag saved ui.paneSizes {sizes}, not a {wanted:.3f} share")
        if len(sizes) != 3 or sizes[2] != 350:
            raise RuntimeError(f"The divider drag changed the assistant entry: {sizes}")

        edit_ui(config, paneSizes=[300, 900], zoomLevel=ZOOM_LEVEL)
        zoomed = driver.drive("zoom-125-dark", [("sleep", 2), ("capture", "zoom-125-dark")])
        zoom = zoomed / "zoom-125-dark.png"
        check(zoom, "My PRs", "Needs Review", "Repositories")
        scaled = measure(zoom, run)
        require_share(scaled, DEFAULT_SHARE, "zoomed")
        # The activity bar is 48px at 100%; ui.zoomLevel scales the shell.
        ratio = bar_width(scaled) / bar_width(launched)
        if not math.isclose(ratio, ZOOM_LEVEL / 100, abs_tol=ZOOM_SLACK):
            raise RuntimeError(f"ui.zoomLevel {ZOOM_LEVEL} scaled the activity bar by {ratio:.3f}")
        return dict(launched=launched, dragged=after, savedSizes=sizes, zoomed=scaled, zoomRatio=ratio)
    finally:
        edit_ui(config, **keep)
