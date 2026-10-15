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
    wanted = [normalize(word) for word in label.split()]
    if not wanted or not all(wanted):
        raise RuntimeError("A nonempty visible label is required")
    lines = {}
    for word in words:
        lines.setdefault(word["line"], []).append(word)
    matches = []
    for line in lines.values():
        line.sort(key=lambda word: word["left"])
        if (field and len(line) > 5) or (exact_line and len(line) != len(wanted)):
            continue
        for start in range(len(line) - len(wanted) + 1):
            span = line[start:start + len(wanted)]
            if [normalize(word["text"]) for word in span] != wanted:
                continue
            left = min(word["left"] for word in span)
            top = min(word["top"] for word in span)
            right = max(word["left"] + word["width"] for word in span)
            bottom = max(word["top"] + word["height"] for word in span)
            matches.append(dict(left=left, top=top, width=right-left, height=bottom-top, exact=len(line) == len(wanted)))
    if field and any(match["exact"] for match in matches):
        matches = [match for match in matches if match["exact"]]
    if navigation:
        heading = find_label(words, "Settings", exact_line=True)
        matches = [match for match in matches if heading["top"] < match["top"] < heading["top"] + heading["height"] + 70]
    if len(matches) != 1:
        raise RuntimeError(f"Expected exactly one visible '{label}' label, observed {len(matches)}")
    matches[0].pop("exact")
    return matches[0]


def recognize(image):
    if shutil.which("tesseract"):
        result = subprocess.run(["tesseract", str(image), "stdout", "--psm", "11", "tsv"], capture_output=True, text=True, check=True, timeout=30)
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--label", required=True)
    parser.add_argument("--navigation", action="store_true")
    parser.add_argument("--field", action="store_true")
    args = parser.parse_args()
    print(json.dumps(locate_label(recognize(args.image), args.label, args.navigation, args.field)))


if __name__ == "__main__":
    main()
