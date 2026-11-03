"""Offline real-engine replay; no app, installer, account or desktop input."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("browser_replay", HERE / "github-desktop.py")
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)
MARKERS = ("PROBE PAGE", "Downloads are blocked", "Bookmarks", "Buddy Probe")

def replay(output):
    output.mkdir(parents=True, exist_ok=False)
    source = HERE / "fixtures/ubuntu-73d-browser-content.png"
    provenance = json.loads(source.with_suffix(".json").read_text())
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    if digest != provenance["sha256"]:
        raise RuntimeError("The retained hosted analysis input changed")
    receipt = dict(source=provenance, sha256=digest,
        engine=subprocess.check_output(["tesseract", "--version"], text=True, timeout=10).splitlines()[0],
        profiles={}, limits="Offline analysis only; original raw Mini check was already PASS. Not installed/runtime or hosted-engine qualification.")
    print(receipt["engine"], flush=True)
    for name, smooth in (("old", False), ("linux", True)):
        folder = output / name
        folder.mkdir()
        image = folder / "capture.png"
        shutil.copyfile(source, image)
        attempts = []
        for candidate in desktop.settings.ocr.analysis_images(image, desktop.settings.run, smooth_inverted=smooth):
            for mode in (11, 3, 6):
                try:
                    desktop.settings.verify_text(candidate, *MARKERS, mode=mode)
                    complete = True
                except desktop.settings.MissingVisibleText:
                    complete = False
                text = candidate.with_suffix(".ocr.txt").read_text(encoding="utf-8")
                attempts.append(dict(image=candidate.name, imageSha256=hashlib.sha256(candidate.read_bytes()).hexdigest(), mode=mode,
                    complete=complete, text=text))
                receipt["profiles"][name] = attempts
                (output / "analysis-replay.json").write_text(json.dumps(receipt, indent=2), encoding="utf-8")
        receipt["profiles"][name] = attempts
    (output / "analysis-replay.json").write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    if not any(row["complete"] for row in receipt["profiles"]["linux"]):
        raise RuntimeError("The target engine cannot read all four captured markers in one Linux layout; receipt retained")
    return receipt

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    value = replay(args.output)
    print(json.dumps({key: [(row["image"], row["mode"]) for row in rows if row["complete"]] for key, rows in value["profiles"].items()}, indent=2))
