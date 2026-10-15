"""Locate a visible settings label in an untouched owned-window screenshot."""
import argparse
import csv
import json
from pathlib import Path
import shutil
import subprocess
import sys


def normalize(text):
    return "".join(character for character in text.casefold() if character.isalnum())


def find_label(words, label, navigation=False, field=False, exact_line=False):
    wanted = normalize(label)
    if not wanted:
        raise RuntimeError("A nonempty visible label is required")
    lines = {}
    for word in words:
        lines.setdefault(word["line"], []).append(word)
    matches = []
    for line in lines.values():
        line.sort(key=lambda word: word["left"])
        if field and len(line) > 5:
            continue
        # A field label starts its OCR line. In particular, a missing Model ID
        # must never fall back to the suffix of Premium model ID.
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
    if field and any(match["exact"] for match in matches):
        matches = [match for match in matches if match["exact"]]
    if navigation:
        heading = find_label(words, "Settings", exact_line=True)
        matches = [match for match in matches if heading["top"] < match["top"] < heading["top"] + heading["height"] + 70]
    if len(matches) != 1:
        raise RuntimeError(f"Expected exactly one visible '{label}' label, observed {len(matches)}")
    matches[0].pop("exact")
    return matches[0]


def recognize(image, mode=11):
    if shutil.which("tesseract"):
        result = subprocess.run(["tesseract", str(image), "stdout", "--psm", str(mode), "tsv"], capture_output=True, text=True, check=True, timeout=30)
        rows = csv.DictReader(result.stdout.splitlines(), delimiter="\t")
        return [dict(text=row["text"], left=int(row["left"]), top=int(row["top"]), width=int(row["width"]), height=int(row["height"]), line="/".join(row[key] for key in ("block_num", "par_num", "line_num"))) for row in rows if row["level"] == "5" and row["text"].strip()]
    if sys.platform == "win32":
        result = subprocess.run(["powershell.exe", "-NoProfile", "-File", str(Path(__file__).with_name("windows-ocr-words.ps1")), "-Path", str(image)], capture_output=True, text=True, check=True, timeout=30)
        return json.loads(result.stdout)
    raise RuntimeError("Native settings qualification requires Tesseract")


def locate_label(words, labels, navigation=False, field=False):
    for label in labels.split("|"):
        try:
            return find_label(words, label, navigation, field)
        except RuntimeError:
            pass
    raise RuntimeError("None of the requested visible labels was unambiguous")


def locate_image(image, labels, navigation=False, field=False):
    for mode in ((11, 3, 6) if shutil.which("tesseract") else (11,)):
        words = recognize(image, mode)
        try:
            return locate_label(words, labels, navigation, field)
        except RuntimeError:
            # Retain the actual native engine output with the already-owned
            # screenshot, so a failure can be diagnosed without guessing.
            image.with_suffix(f".ocr-{mode}.json").write_text(json.dumps(words), encoding="utf-8")
    raise RuntimeError(f"No OCR mode found the visible label {labels!r} unambiguously")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--label", required=True)
    parser.add_argument("--navigation", action="store_true")
    parser.add_argument("--field", action="store_true")
    args = parser.parse_args()
    print(json.dumps(locate_image(args.image, args.label, args.navigation, args.field)))


if __name__ == "__main__":
    main()
