"""Locate a visible settings label in an untouched owned-window screenshot."""
import argparse
import csv
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys


class AmbiguousLabel(RuntimeError):
    """A later OCR mode must not hide competing visible controls."""


def normalize(text):
    return "".join(character for character in text.casefold() if character.isalnum())


def find_label(words, label, navigation=False, field=False, exact_line=False, tab=False, content=False):
    wanted = normalize(label)
    if not wanted:
        raise RuntimeError("A nonempty visible label is required")
    # Borders and punctuation have no label characters. Keeping them in a
    # span can count one word twice, such as Interrupt and Interrupt + '('.
    words = [word for word in words if normalize(word["text"])]
    if content:
        # Explicit scene scope: the menu row and the following workspace-tab
        # row are chrome, not controls in the retained view. Measure both from
        # the untouched capture rather than assuming a DPI or window origin.
        menu = find_label(words, "File")
        help_label = find_label(words, "Help")
        if abs(menu["top"] - help_label["top"]) > max(menu["height"], help_label["height"]):
            raise RuntimeError("The native menu row is not visible")
        below_menu = [word for word in words if word["top"] > menu["top"] + menu["height"]]
        if not below_menu:
            raise RuntimeError("The workspace tab row is not visible")
        row_top = min(word["top"] for word in below_menu)
        if row_top > menu["top"] + 5 * menu["height"]:
            raise RuntimeError("The workspace tab row is not visible")
        row = [word for word in below_menu if word["top"] < row_top + menu["height"]]
        row_bottom = max(word["top"] + word["height"] for word in row)
        words = [word for word in words if word["top"] > row_bottom]
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
    result = subprocess.run(["powershell.exe", "-NoProfile", "-File", str(Path(__file__).with_name("windows-ocr-words.ps1")), "-Path", str(image)], capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
    return json.loads(result.stdout)


def recognize_macos(image):
    helper = Path(os.environ.get("BUDDY_SETTINGS_OCR_HELPER", ""))
    if not helper.is_absolute() or not helper.is_file():
        raise RuntimeError("macOS OCR requires an existing absolute compiled helper")
    result = subprocess.run([str(helper), str(image)],
        capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
    return json.loads(result.stdout)


def recognize(image, mode=11):
    if shutil.which("tesseract"):
        result = subprocess.run(["tesseract", str(image), "stdout", "--psm", str(mode), "tsv"], capture_output=True, text=True, encoding="utf-8", check=True, timeout=30)
        rows = csv.DictReader(result.stdout.splitlines(), delimiter="\t", quoting=csv.QUOTE_NONE)
        return [dict(text=row["text"], left=int(row["left"]), top=int(row["top"]), width=int(row["width"]), height=int(row["height"]), line="/".join(row[key] for key in ("block_num", "par_num", "line_num"))) for row in rows if row["level"] == "5" and row["text"].strip()]
    if sys.platform == "win32":
        return recognize_windows(image)
    raise RuntimeError("Native settings qualification requires Tesseract")


def locate_label(words, labels, navigation=False, field=False, tab=False, content=False):
    for label in labels.split("|"):
        try:
            return find_label(words, label, navigation, field, tab=tab, content=content)
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


def locate_image(image, labels, navigation=False, field=False, save_row=False, tab=False, row_control=None, block_first=False, content=False):
    # Compact Mac forms put a focus border just above the next label. Sparse
    # segmentation can absorb that border into its glyph box (#886). The
    # original-image block pass reads the label alone in the failed artifacts.
    modes = (6, 11, 3) if block_first else (11, 3, 6)
    for mode in (modes if shutil.which("tesseract") else (11,)):
        words = recognize(image, mode)
        try:
            label = locate_label(words, labels, navigation, field, tab, content=content)
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
                        capture_output=True, check=True, timeout=30)
                raw = recognize(analysis, mode)
                words = [dict(word, **{key: word[key] / 3 for key in ("left", "top", "width", "height")}) for word in raw]
                try:
                    label = locate_label(words, labels, navigation, field, tab, content=content)
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
            label = locate_label(words, labels, navigation, field, tab, content=content)
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
    args = parser.parse_args()
    print(json.dumps(locate_image(args.image, args.label, args.navigation, args.field, tab=args.tab, content=args.content)))


if __name__ == "__main__":
    main()
