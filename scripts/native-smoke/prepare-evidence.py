"""Stage regular evidence files without following disposable fixture links."""
import argparse
import json
import os
from pathlib import Path
import shutil


def stage(source, destination):
    source, destination = source.absolute(), destination.absolute()
    if destination.resolve().is_relative_to(source.resolve()):
        raise ValueError("Upload staging must be outside the evidence tree")
    destination.mkdir(parents=True, exist_ok=False)
    skipped = []
    count = 0

    def visit(directory, target):
        nonlocal count
        for entry in os.scandir(directory):
            relative = str(Path(entry.path).relative_to(source))
            if entry.is_symlink() or (os.name == "nt" and Path(entry.path).is_junction()):
                skipped.append(relative)
            elif entry.is_dir(follow_symlinks=False):
                child = target / entry.name
                child.mkdir()
                visit(Path(entry.path), child)
            elif entry.is_file(follow_symlinks=False):
                shutil.copy2(entry.path, target / entry.name, follow_symlinks=False)
                count += 1
            else:
                skipped.append(relative)

    if source.is_symlink():
        raise ValueError("The evidence root must not be a link")
    if source.exists():
        visit(source, destination)
    receipt = dict(files=count, skippedLinksOrSpecialFiles=sorted(skipped))
    (destination / "upload-staging.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(receipt))
    return receipt


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    stage(args.source, args.destination)
