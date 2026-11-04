"""Locate a visible settings label in an untouched owned-window screenshot."""
import argparse
import csv
import json
import math
import os
import struct
from pathlib import Path
import shutil
import subprocess
import sys


# shell/status_bar.rs uses this height at 100% native zoom.
STATUS_BAR_HEIGHT = 22.0

class AmbiguousLabel(RuntimeError):
    """A later OCR mode must not hide competing visible controls."""


def normalize(text):
    return "".join(character for character in text.casefold() if character.isalnum())


def status_footer(words):
    """Measure the unique ordered totals row at the bottom of an OCR layout."""
    bottom = max((word["top"] + word["height"] for word in words), default=0)
    def status_label(word, label):
        text = normalize(word["text"])
        if text == label:
            return True
        # The captured Windows OCR joins zero and jobs as "Ojobs".
        prefix = text[:-len(label)] if text.endswith(label) else ""
        return bool(prefix) and all(c.isdecimal() or c == "o" for c in prefix)

    footers = []
    for candidate in (word for word in words if status_label(word, "prs")):
        row = [word for word in words if abs(word["top"] - candidate["top"])
            <= max(word["height"], candidate["height"]) / 2]
        labels = [[word for word in row if status_label(word, label)]
            for label in ("prs", "schedules", "jobs")]
        if not all(len(matches) == 1 for matches in labels):
            continue
        positions = [matches[0]["left"] for matches in labels]
        height = max(word["height"] for word in row)
        if positions == sorted(set(positions)) and candidate["top"] >= bottom - 2 * height:
            footers.append(dict(top=min(word["top"] for word in row), height=height,
                left=min(matches[0]["left"] for matches in labels),
                right=max(matches[0]["left"] + matches[0]["width"] for matches in labels)))
    if len(footers) > 1:
        raise AmbiguousLabel("More than one native status footer is visible")
    return footers[0] if footers else None


def analysis_images(path, run, *, smooth_inverted=False):
    yield path
    # Analysis copy only; the original window capture is never edited.
    image = path.with_suffix(".ocr-scale3.png")
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", "scale=iw*3:ih*3:flags=neighbor", "-frames:v", "1", image)
    yield image
    # Mixed dark shell chrome and a light hosted page can segment separately.
    # Keep one-layout matching, but offer the same contrast analysis as locators.
    image = path.with_suffix(".ocr-scale2-inverted.png" if smooth_inverted else ".ocr-scale3-inverted.png")
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", ("scale=iw*2:ih*2:flags=lanczos,format=gray,negate" if smooth_inverted
         else "scale=iw*3:ih*3:flags=neighbor,format=gray,negate"), "-frames:v", "1", image)
    yield image


def crop_content_image(path, bottom, run, suffix=".content-ocr.png"):
    height = struct.unpack(">II", path.read_bytes()[16:24])[1]
    if not 0 < bottom < height:
        raise RuntimeError("The status footer leaves invalid content bounds")
    image = path.with_suffix(suffix)
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", f"crop=iw:{bottom}:0:0:exact=1", "-frames:v", "1", image)
    return image


def known_footer_top(path, footer, scale, zoom=1.0):
    """Use the native layout contract only with an owned capture's scale receipt."""
    receipt = path.with_suffix(".window.json")
    if not receipt.is_file():
        return None
    try:
        metadata = json.loads(receipt.read_text(encoding="utf-8"))
        width, height = struct.unpack(">II", path.read_bytes()[16:24])
        backing = metadata["capturePixelsPerWindowPoint"]
        bounds = metadata["kCGWindowBounds"]
        numbers = (backing, zoom, scale, bounds["Width"], bounds["Height"])
        if any(isinstance(value, bool) or not isinstance(value, (int, float))
                or not math.isfinite(value) or value <= 0 for value in numbers):
            raise ValueError("Invalid scale")
        if metadata["capturePixelSize"] != [width, height] or not all((
                math.isclose(bounds["Width"] * backing, width, abs_tol=1),
                math.isclose(bounds["Height"] * backing, height, abs_tol=1))):
            raise ValueError("Mismatched capture dimensions")
        top = round(height - STATUS_BAR_HEIGHT * backing * zoom)
        band_top = footer["top"] / scale
        band_bottom = (footer["top"] + footer["height"]) / scale
        if not (0 < top < height and 0 <= band_top < band_bottom <= height
                and band_bottom > top and band_top >= top - (height - top)):
            raise ValueError("The measured text does not overlap the known footer")
        return top
    except (KeyError, TypeError, ValueError, OverflowError, struct.error) as error:
        raise RuntimeError("The native footer fallback has invalid capture metadata") from error


def footer_border(path, footer, scale, run, zoom=1.0):
    """Find the painted separator above the measured labels, across their span."""
    left = math.floor(footer["left"] / scale)
    right = math.ceil(footer["right"] / scale)
    top = math.floor((footer["top"] - 2 * footer["height"]) / scale)
    end = math.floor(footer["top"] / scale)
    width, height = struct.unpack(">II", path.read_bytes()[16:24])
    if not (0 <= left < right <= width and 0 <= top < end <= height):
        raise RuntimeError("The status footer has invalid capture bounds")
    raw = path.with_suffix(".footer-rgb")
    run("ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", path,
        "-vf", f"crop={right-left}:{end-top}:{left}:{top}:exact=1,format=rgb24",
        "-f", "rawvideo", "-frames:v", "1", raw)
    pixels = raw.read_bytes()
    stride = (right - left) * 3
    if len(pixels) != stride * (end - top):
        raise RuntimeError("The footer analysis has invalid pixel dimensions")
    previous_uniform = True
    for offset in range(end - top):
        row = pixels[offset * stride:(offset + 1) * stride]
        uniform = row == row[:3] * (right - left)
        if uniform and not previous_uniform:
            return top + offset
        previous_uniform = uniform
    # OCR may absorb the painted border into a label, ending the scan above
    # the line. A validated backing-scale receipt can still bound the footer.
    fallback = known_footer_top(path, footer, scale, zoom)
    if fallback is not None:
        return fallback
    raise RuntimeError("The measured native status footer has no visible separator")


def status_content_image(path, run, suffix=".content-ocr.png", border=False, zoom=1.0, smooth_inverted=False):
    """Measure and exclude the shared footer from an untouched capture."""
    width = struct.unpack(">II", path.read_bytes()[16:24])[0]
    for analysis in analysis_images(path, run, smooth_inverted=smooth_inverted):
        scale = struct.unpack(">II", analysis.read_bytes()[16:24])[0] / width
        for mode in (11, 3, 6):
            words = recognize(analysis, mode=mode)
            analysis.with_suffix(f".footer-ocr-{mode}.json").write_text(json.dumps(words), encoding="utf-8")
            footer = status_footer(words)
            if footer:
                bottom = (footer_border(path, footer, scale, run, zoom) if border else
                    int((footer["top"] - footer["height"] / 2) / scale))
                return crop_content_image(path, bottom, run, suffix)
    raise RuntimeError("The content screenshot has no unique native status footer")


def chrome_rows(words):
    """Measure the menu bottom and following workspace-tab row bottom."""
    chrome = words
    chrome = [word for word in chrome if normalize(word["text"])]
    menus = []
    for candidate in (word for word in chrome if normalize(word["text"]) == "file"):
        row = [word for word in chrome if abs(word["top"] - candidate["top"])
            <= max(word["height"], candidate["height"]) / 2]
        labels = [[word for word in row if normalize(word["text"]) == label]
            for label in ("file", "view", "help")]
        if all(len(matches) == 1 for matches in labels):
            positions = [matches[0]["left"] for matches in labels]
            if positions == sorted(set(positions)):
                menus.append(candidate)
    if len(menus) > 1:
        raise AmbiguousLabel("More than one native menu row is visible")
    if not menus:
        raise RuntimeError("The native File/Edit/View/Help menu row is not visible")
    menu = menus[0]
    below_menu = [word for word in chrome if word["top"] > menu["top"] + menu["height"]]
    if not below_menu:
        raise RuntimeError("The workspace tab row is not visible")
    row_top = min(word["top"] for word in below_menu)
    if row_top > menu["top"] + 5 * menu["height"]:
        raise RuntimeError("The workspace tab row is not visible")
    row = [word for word in below_menu if word["top"] < row_top + menu["height"]]
    row_bottom = max(word["top"] + word["height"] for word in row)
    return menu["top"] + menu["height"], row_bottom


def workspace_words(words):
    """Keep workspace titles above the view and below the custom menu."""
    top, bottom = chrome_rows(words)
    return [word for word in words if top < word["top"] <= bottom]


def workspace_geometry(image):
    """Measure the shell's two painted bands when OCR cannot read its menu.

    The blank right portion of the title and tab bars spans the sampled strip.
    Require two adjacent, similarly sized uniform bands starting at the window
    top; clipped, textured or incomplete chrome must fail closed.
    """
    width, height = struct.unpack(">II", image.read_bytes()[16:24])
    scan = height // 3
    result = subprocess.run(["ffmpeg", "-nostdin", "-loglevel", "error", "-i", str(image),
        "-vf", f"crop={width//4}:{scan}:{width*3//5}:0:exact=1,scale=5:{scan}:flags=neighbor,format=rgb24",
        "-f", "rawvideo", "-frames:v", "1", "pipe:1"], capture_output=True, check=True, timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    pixels = result.stdout
    image.with_suffix('.workspace-bands.rgb').write_bytes(pixels)
    if len(pixels) != scan * 15:
        raise RuntimeError("The workspace geometry has invalid pixel dimensions")
    keys = [pixels[y*15:y*15+3] if pixels[y*15:(y+1)*15] == pixels[y*15:y*15+3]*5 else None for y in range(scan)]
    runs, start = [], 0
    for end in range(1, scan + 1):
        if end == scan or keys[end] != keys[start]:
            if keys[start] is not None and end - start >= 8:
                runs.append((start, end))
            start = end
    if len(runs) < 2:
        raise RuntimeError("The workspace has no measured chrome bands")
    menu, tabs = runs[:2]
    menu_height, tab_height = menu[1] - menu[0], tabs[1] - tabs[0]
    if not (menu[0] <= 2 and 0 < tabs[0] - menu[1] <= 3 and .85 <= tab_height/menu_height <= 1.2):
        raise RuntimeError("The workspace chrome bands do not match the shell layout")
    image.with_suffix('.workspace-band.json').write_text(json.dumps(dict(
        captureSize=[width, height], menu=menu, workspace=tabs)), encoding='utf-8')
    return tabs


def workspace_target(image, labels):
    engine = recognize_windows if sys.platform == 'win32' else lambda path: recognize(path, 11)
    words = engine(image)
    if image.is_file():
        image.with_suffix('.workspace-ocr.json').write_text(json.dumps(words), encoding='utf-8')
    try:
        return locate_label(workspace_words(words), labels)
    except AmbiguousLabel:
        raise
    except RuntimeError:
        top, bottom = workspace_geometry(image)
        cropped = image.with_suffix('.workspace-band.png')
        subprocess.run(["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", str(image),
            "-vf", f"crop=iw:{bottom-top}:0:{top}:exact=1", "-frames:v", "1", str(cropped)],
            capture_output=True, check=True, timeout=30,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        words = engine(cropped)
        cropped.with_suffix('.ocr.json').write_text(json.dumps(words), encoding='utf-8')
        label = locate_label(words, labels)
        return dict(label, top=label['top'] + top)


def content_words(words, chrome=None):
    """Keep view controls below workspace tabs and above status totals."""
    _, row_bottom = chrome_rows(words if chrome is None else chrome)
    words = [word for word in words if word["top"] > row_bottom]
    # The shared totals footer is chrome too. Identify its three ordered
    # labels on the bottom OCR row, never by a fixed pixel/DPI cutoff.
    # Older captures without totals retain the existing upper-bound scope.
    footer = status_footer(words)
    if footer:
        words = [word for word in words if word["top"] < footer["top"]]
    return words


def find_label(words, label, navigation=False, field=False, exact_line=False, tab=False, content=False, between=None):
    wanted = normalize(label)
    if not wanted:
        raise RuntimeError("A nonempty visible label is required")
    # Borders and punctuation have no label characters. Keeping them in a
    # span can count one word twice, such as Interrupt and Interrupt + '('.
    words = [word for word in words if normalize(word["text"])]
    if content:
        words = content_words(words)
    if between is not None:
        # A scene may name the two visible field labels bounding a row.
        # Use their tops: OCR can absorb an input border into a label's height.
        # Missing/competing anchors or competing controls still fail closed.
        start, end = (find_label(words, text, field=True) for text in between)
        if start["top"] >= end["top"]:
            raise RuntimeError("The visible row boundaries are out of order")
        words = [word for word in words if start["top"] < word["top"] < end["top"]]
    if field:
        try:
            heading = find_label(words, "Settings", exact_line=True)
            # Glyph bounding boxes differ slightly even at the same layout x.
            # The recorded heading S starts at 81 while Accent's A starts at 80.
            content_left = heading["left"] - max(4, heading["height"] // 2)
            words = [word for word in words if word["left"] >= content_left]
        except RuntimeError:
            pass
    lines = {}
    for word in words:
        lines.setdefault(word["line"], []).append(word)
    matches = []
    groups = []
    for line in lines.values():
        line.sort(key=lambda word: word["left"])
        group = []
        for word in line:
            if group:
                previous = group[-1]
                gap = word["left"] - previous["left"] - previous["width"]
                if gap > max(24, 2 * max(previous["height"], word["height"])):
                    groups.append(group)
                    group = []
            group.append(word)
        if group:
            groups.append(group)
    # Field labels are short, so longer groups are prose. A requested label
    # with more words still bounds its own group, never a longer line.
    longest = max(5, len(label.split()))
    for line in groups:
        if field and len(line) > longest:
            continue
        # A field label starts its spatial group. OCR may combine the sidebar
        # and both account columns into one line. A missing Model ID must still
        # never fall back to the adjacent words in Premium model ID.
        for start in ([0] if field else range(len(line))):
            for end in range(start + 1, len(line) + 1):
                span = line[start:end]
                if "".join(normalize(word["text"]) for word in span) != wanted:
                    continue
                exact = start == 0 and end == len(line)
                if exact_line and not exact:
                    continue
                left = min(word["left"] for word in span)
                top = min(word["top"] for word in span)
                right = max(word["left"] + word["width"] for word in span)
                bottom = max(word["top"] + word["height"] for word in span)
                matches.append(dict(left=left, top=top, width=right-left, height=bottom-top, exact=exact))
    if navigation:
        heading = find_label(words, "Settings", exact_line=True)
        matches = [match for match in matches if heading["top"] < match["top"] < heading["top"] + heading["height"] + 70]
    if tab:
        anchor = find_label(words, "Filter loaded items", field=True)
        bottom = anchor["top"] + anchor["height"]
        matches = [match for match in matches if bottom < match["top"] < bottom + 16 * anchor["height"]]
        if matches:
            first = min(match["top"] for match in matches)
            matches = [match for match in matches if match["top"] < first + max(2, anchor["height"] // 2)]
    # A complete spatial label identifies the control more precisely than
    # the same word embedded in status/help prose. Two complete labels still
    # fail below; never resolve competing controls by their discovery order.
    if any(match["exact"] for match in matches):
        matches = [match for match in matches if match["exact"]]
    if len(matches) > 1:
        raise AmbiguousLabel(f"Cannot locate '{label}' unambiguously: observed {len(matches)} visible matches")
    if not matches:
        raise RuntimeError(f"Expected exactly one visible '{label}' label, observed {len(matches)}")
    matches[0].pop("exact")
    return matches[0]


def recognize_windows(image):
    # Captured pipes do not stop a console child from opening a terminal on
    # Windows. OCR is repeatedly invoked by hidden-desktop smoke workers.
    result = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-File", str(Path(__file__).with_name("windows-ocr-words.ps1")), "-Path", str(image)], capture_output=True, text=True, encoding="utf-8", check=True, timeout=30,
        creationflags=subprocess.CREATE_NO_WINDOW)
    return json.loads(result.stdout)


def recognize_macos(image):
    helper = Path(os.environ.get("BUDDY_SETTINGS_OCR_HELPER", ""))
    if not helper.is_absolute() or not helper.is_file():
        raise RuntimeError("macOS OCR requires an existing absolute compiled helper")
    try:
        result = subprocess.run([str(helper), str(image)],
            capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
    except subprocess.CalledProcessError as error:
        # Preserve the exact failed capture before the status wait recaptures it.
        shutil.copyfile(image, image.with_suffix(".ocr-error.png"))
        details = dict(exitCode=error.returncode, stderr=error.stderr or "")
        image.with_suffix(".ocr-error.json").write_text(json.dumps(details), encoding="utf-8")
        raise RuntimeError(f"macOS OCR helper exited {error.returncode}: {details['stderr'].strip()}") from error
    return json.loads(result.stdout)


def recognize(image, mode=11):
    if shutil.which("tesseract"):
        result = subprocess.run(["tesseract", str(image), "stdout", "--psm", str(mode), "tsv"], capture_output=True, text=True, encoding="utf-8", check=True, timeout=30,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        rows = csv.DictReader(result.stdout.splitlines(), delimiter="\t", quoting=csv.QUOTE_NONE)
        return [dict(text=row["text"], left=int(row["left"]), top=int(row["top"]), width=int(row["width"]), height=int(row["height"]), line="/".join(row[key] for key in ("block_num", "par_num", "line_num"))) for row in rows if row["level"] == "5" and row["text"].strip()]
    if sys.platform == "win32":
        return recognize_windows(image)
    raise RuntimeError("Native settings qualification requires Tesseract")


def locate_label(words, labels, navigation=False, field=False, tab=False, content=False, between=None):
    for label in labels.split("|"):
        try:
            return find_label(words, label, navigation, field, tab=tab, content=content, between=between)
        except AmbiguousLabel:
            raise
        except RuntimeError:
            pass
    raise RuntimeError("None of the requested visible labels was unambiguous")


def column_text(words, anchor):
    """Read one visible column below its unique label, preserving word order.

    A flat OCR page can insert a neighboring list between wrapped text lines.
    Use measured label geometry to exclude that list, rather than accepting
    partial phrases or combining independent OCR results.
    """
    label = find_label(words, anchor)
    left = label["left"] - max(2, label["height"] / 4)
    top = label["top"] - max(2, label["height"] / 4)
    lines = {}
    for word in words:
        if word["left"] >= left and word["top"] >= top:
            lines.setdefault(word["line"], []).append(word)
    ordered = sorted(lines.values(), key=lambda line: min(word["top"] for word in line))
    return " ".join(" ".join(word["text"] for word in sorted(line, key=lambda word: word["left"]))
                    for line in ordered)


def aligned_save(words, label):
    """Find the visible button aligned with a single-column field's editor."""
    candidates = [word for word in words if normalize(word["text"]) == "save"
        and word["left"] > label["left"] + label["width"] + 4 * label["height"]
        and label["top"] + label["height"] < word["top"] < label["top"] + max(64, 6 * label["height"])]
    if not candidates:
        raise RuntimeError("The field's aligned Save button is not visible")
    nearest = min(word["top"] for word in candidates)
    candidates = [word for word in candidates if word["top"] < nearest + label["height"]]
    if len(candidates) != 1:
        raise RuntimeError("The field's aligned Save button is ambiguous")
    return candidates[0]


def aligned_control(words, label, text):
    """Require the complete adjacent button in the field's measured row."""
    candidates = [word for word in words
        if word["left"] > label["left"] + label["width"] + 4 * label["height"]
        and label["top"] + label["height"] < word["top"] < label["top"] + max(64, 6 * label["height"])]
    return find_label(candidates, text)


def input_region(label, dimensions):
    """Use measured glyph pixels to exclude the input's focus border."""
    button = label["save"]
    height = button["height"]
    if not math.isfinite(height) or height <= 0:
        raise RuntimeError("A positive measured control height is required")
    center = button["top"] + height / 2
    left = math.ceil(label["left"] + height * 0.4)
    right = math.floor(button["left"] - height * 2.4)
    top, bottom = math.floor(center - height), math.ceil(center + height)
    width, capture_height = dimensions
    if not (0 <= left < right <= width and 0 <= top < bottom <= capture_height
            and right - left >= height * 4):
        raise RuntimeError("The visible input has invalid capture bounds")
    return dict(left=left, top=top, width=right-left, height=bottom-top)


def color_region(label, scale, dimensions):
    """Keep the original pixels of the color row, excluding other controls."""
    if not math.isfinite(scale) or scale <= 0:
        raise RuntimeError("A positive measured capture scale is required")
    button = label["save"]
    center = button["top"] + button["height"] / 2
    left, right = int(label["left"]), int(button["left"] - 20 * scale)
    top, bottom = math.floor(center - 12 * scale), math.ceil(center + 12 * scale)
    width, height = dimensions
    if not (0 <= left < right <= width and 0 <= top < bottom <= height
            and right - left >= 40 * scale):
        raise RuntimeError("The visible color input has invalid capture bounds")
    return dict(left=left, top=top, width=right-left, height=bottom-top)


def locate_image(image, labels, navigation=False, field=False, save_row=False, tab=False, row_control=None, block_first=False, content=False, between=None, native_macos=False, workspace=False, sparse_only=False):
    if sparse_only:
        words = recognize(image, 11)
        image.with_suffix('.sparse-ocr.json').write_text(json.dumps(words), encoding='utf-8')
        return locate_label(words, labels, navigation, field, tab, content=content, between=between)
    if workspace:
        # Global tab titles are shell chrome, not view controls. Keep their
        # measurements on one engine per platform. Sparse Tesseract reads
        # Mac titles that Vision replaces with Cyrillic lookalikes; Windows
        # OCR reads the 100% DPI titles that sparse Tesseract fragments.
        return workspace_target(image, labels)
    if native_macos:
        # Stability measurements must not alternate Tesseract glyph boxes
        # with Vision's line boxes for the same stationary control.
        words = recognize_macos(image)
        # Preserve missing/ambiguous native tokens too, before matching can
        # raise; raw captures alone cannot reproduce another OS's Vision model.
        if image.is_file():
            image.with_suffix('.ocr-darwin.json').write_text(json.dumps(words), encoding='utf-8')
        if content:
            # Vision on Intel can omit the File menu while reading every view
            # control. A missing header may use Tesseract's chrome boundary,
            # but target boxes always remain Vision's. Competing headers must
            # still fail rather than being hidden by another engine.
            try:
                words = content_words(words)
            except AmbiguousLabel:
                raise
            except RuntimeError:
                chrome = recognize(image)
                if image.is_file():
                    image.with_suffix('.chrome-ocr.json').write_text(json.dumps(chrome), encoding='utf-8')
                words = content_words(words, chrome)
        label = locate_label(words, labels, navigation, field, tab, content=False, between=between)
        if save_row:
            label["save"] = aligned_save(words, label)
        if row_control is not None:
            label["row_control"] = aligned_control(words, label, row_control)
        return label
    # Compact Mac forms put a focus border just above the next label. Sparse
    # segmentation can absorb that border into its glyph box (#886). The
    # original-image block pass reads the label alone in the failed artifacts.
    modes = (6, 11, 3) if block_first else (11, 3, 6)
    for mode in (modes if shutil.which("tesseract") else (11,)):
        words = recognize(image, mode)
        try:
            label = locate_label(words, labels, navigation, field, tab, content=content, between=between)
            if save_row:
                label["save"] = aligned_save(words, label)
            if row_control is not None:
                label["row_control"] = aligned_control(words, label, row_control)
            return label
        except RuntimeError as error:
            # Retain the actual native engine output with the already-owned
            # screenshot, so a failure can be diagnosed without guessing.
            image.with_suffix(f".ocr-{mode}.json").write_text(json.dumps(words), encoding="utf-8")
            if isinstance(error, AmbiguousLabel):
                raise
    if shutil.which("tesseract"):
        # Keep the original capture. Exact 3x analysis copies handle small
        # glyphs, then light text on a colored control (a primary button,
        # which Tesseract misses in the light theme) as dark text after an
        # inverted grayscale pass. Sparse segmentation can still skip a lone
        # button beside a toolbar (light Add in Bookmarks on Ubuntu), so both
        # copies are then read again as one uniform block. All label matching
        # and returned bounds stay in window pixels.
        for mode in (11, 6):
            for suffix, filters in ((".ocr-analysis-3x.png", "scale=iw*3:ih*3:flags=neighbor"),
                    (".ocr-analysis-3x-inverted.png", "scale=iw*3:ih*3:flags=neighbor,format=gray,negate")):
                analysis = image.with_suffix(suffix)
                if mode == 11:
                    subprocess.run(["ffmpeg", "-nostdin", "-y", "-loglevel", "error", "-i", str(image),
                        "-vf", filters, "-frames:v", "1", str(analysis)],
                        capture_output=True, check=True, timeout=30,
                        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                raw = recognize(analysis, mode)
                words = [dict(word, **{key: word[key] / 3 for key in ("left", "top", "width", "height")}) for word in raw]
                try:
                    label = locate_label(words, labels, navigation, field, tab, content=content, between=between)
                    if save_row:
                        label["save"] = aligned_save(words, label)
                    if row_control is not None:
                        label["row_control"] = aligned_control(words, label, row_control)
                    return label
                except RuntimeError as error:
                    analysis.with_suffix(f".ocr-{mode}.json").write_text(json.dumps(raw), encoding="utf-8")
                    if isinstance(error, AmbiguousLabel):
                        raise
        if sys.platform in ("win32", "darwin"):
            # Native OCR can read controls that Tesseract's sparse
            # segmentation misses. Use the untouched capture and identical
            # matching rules, never a guessed point or a relaxed label.
            engine = recognize_windows if sys.platform == "win32" else recognize_macos
            words = engine(image)
            image.with_suffix(f".ocr-{sys.platform}.json").write_text(json.dumps(words), encoding="utf-8")
            label = locate_label(words, labels, navigation, field, tab, content=content, between=between)
            if save_row:
                label["save"] = aligned_save(words, label)
            if row_control is not None:
                label["row_control"] = aligned_control(words, label, row_control)
            return label
    raise RuntimeError(f"No OCR mode found the visible label {labels!r} unambiguously")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--label", required=True)
    parser.add_argument("--navigation", action="store_true")
    parser.add_argument("--field", action="store_true")
    parser.add_argument("--tab", action="store_true")
    parser.add_argument("--content", action="store_true")
    parser.add_argument("--between-labels", nargs=2)
    parser.add_argument("--workspace", action="store_true")
    args = parser.parse_args()
    print(json.dumps(locate_image(args.image, args.label, args.navigation, args.field, tab=args.tab, content=args.content, between=args.between_labels, workspace=args.workspace)))


if __name__ == "__main__":
    main()
