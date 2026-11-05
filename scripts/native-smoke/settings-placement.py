"""One guarded ARM placement pair; original settings failures remain failures."""
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import plistlib
import re
import subprocess
import sys
import time

MODE = "initial-x-v1"
SIZE = (1200, 820)
ORIGINS = ((80, 80), (240, 80))
SOUND_FAILURE = "The owned window did not show the expected status 'Review completion sound: on'"


class AdmissionError(RuntimeError):
    pass


def guard():
    if (sys.platform != "darwin" or platform.machine() != "arm64"
            or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1"):
        raise AdmissionError("Placement comparison requires disposable ARM Mac settings")


def owned_path(path, *, exists=True):
    root = Path(os.environ["RUNNER_TEMP"]).resolve(strict=True)
    path = Path(path).absolute()
    if path.resolve(strict=exists) != path or not path.is_relative_to(root) or path == root:
        raise AdmissionError("Placement path is not canonical owned temporary data")
    return path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def provenance(executable, package, checkout):
    guard()
    executable = owned_path(executable)
    if (not executable.is_file() or executable.name != "buddy-native"
            or executable.parent.name != "MacOS" or executable.parent.parent.name != "Contents"):
        raise AdmissionError("Placement executable is not the installed app")
    info = plistlib.loads((executable.parent.parent / "Info.plist").read_bytes())
    package = Path(package).resolve(strict=True)
    manifest = json.loads((package / "build.json").read_text())
    artifact = package / manifest["package"]["file"]
    if (artifact.parent != package or artifact.is_symlink() or not artifact.is_file()
            or manifest.get("sourceDirty") is not False or manifest.get("profile") != "release"
            or not re.fullmatch(r"[0-9a-f]{40}", manifest.get("sourceSha", ""))
            or digest(artifact) != manifest["package"]["sha256"]
            or not re.fullmatch(r"[0-9a-f]{64}", manifest.get("binarySha256", ""))
            or info.get("CFBundleIdentifier") != "com.hemsoft.buddy.native"
            or info.get("CFBundleShortVersionString") != manifest["version"]):
        raise AdmissionError("Placement installed package provenance mismatch")
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, text=True).strip()
    if source != manifest["sourceSha"]:
        raise AdmissionError("Placement source differs from package")
    return dict(sourceSha=source, version=manifest["version"],
                packageSha256=digest(artifact), stagedBinarySha256=manifest["binarySha256"],
                binarySha256=digest(executable))


def rectangle(value):
    if (not isinstance(value, dict) or set(value) != {"x", "y", "width", "height"}
            or any(type(v) not in (int, float) or not math.isfinite(v) for v in value.values())
            or value["width"] <= 0 or value["height"] <= 0):
        raise AdmissionError("Placement rectangle unavailable")
    return value


def contains(outer, inner):
    return (outer["x"] <= inner["x"] and outer["y"] <= inner["y"]
            and inner["x"] + inner["width"] <= outer["x"] + outer["width"]
            and inner["y"] + inner["height"] <= outer["y"] + outer["height"])


def display_geometry(display):
    if (display.get("space") != "cg-top-left-points" or display.get("main") is not True
            or display.get("single") is not True):
        raise AdmissionError("Placement display admission unavailable")
    bounds, usable = rectangle(display.get("bounds")), rectangle(display.get("usable"))
    scale = display.get("scale")
    if (type(scale) not in (int, float) or not math.isfinite(scale) or scale <= 0
            or not contains(bounds, usable) or bounds["x"] != 0 or bounds["y"] != 0):
        raise AdmissionError("Placement display geometry ambiguous")
    for x, y in ORIGINS:
        if not contains(usable, dict(x=x, y=y, width=SIZE[0], height=SIZE[1])):
            raise AdmissionError("Placement pair does not fit usable main display")


def display_receipt():
    guard()
    path = owned_path(os.environ["BUDDY_SETTINGS_DISPLAY_RECEIPT"])
    display = json.loads(path.read_text()).get("comparisonDisplay", {})
    display_geometry(display)
    return display


class Arm:
    def __init__(self, origin, executable=None):
        if origin not in ORIGINS:
            raise AdmissionError("Unknown placement arm")
        self.origin = origin
        self.executable = executable
        self.window = None
        self.config = None
        self.assigned = False
        self.display = None
        self.window_id = None
        self.targets = []
        self.cleanup = []
        self.healthy = True

    def attach(self, window, config, *, scene):
        guard()
        if scene != "preferences":
            raise AdmissionError("Placement is preferences-only")
        config = owned_path(config)
        process = window.process
        if (not config.is_file() or config.name != "config.json"
                or (config.parent / ".buddy-settings-fixture").read_text() != "buddy settings fixture v1"
                or not isinstance(process, subprocess.Popen) or process.poll() is not None
                or window.session != "macos"):
            raise AdmissionError("Placement requires live owned fixture window")
        if owned_path(Path(process.args[0])) != self.executable:
            raise AdmissionError("Placement executable ownership mismatch")
        self.window, self.config = window, config
        window._placement = self

    def geometry(self, window, origin, size):
        guard()
        if (window is not self.window or window.process.poll() is not None
                or tuple(origin) != self.origin or tuple(size) != SIZE):
            raise AdmissionError("Placement geometry refused before AX operation")
        display = display_receipt()
        if self.display is not None and self.display != display:
            raise AdmissionError("Placement admission display changed")
        self.display = display
        return f"set position of window 1 to {{{origin[0]}, {origin[1]}}}"

    def assigned_geometry(self, actual):
        if actual != [*SIZE, *self.origin]:
            raise AdmissionError("Owned AX placement was not applied")
        self.assigned = True

    def capture_environment(self):
        guard()
        if not self.assigned or self.window.process.poll() is not None:
            raise AdmissionError("Placement capture requires assigned live window")
        return dict(os.environ, BUDDY_SETTINGS_COMPARISON=MODE, BUDDY_CONFIG_PATH=str(self.config))

    def capture(self, metadata):
        if self.window.process.poll() is not None:
            raise AdmissionError("Placement owned app exited")
        display = metadata.get("comparisonDisplay", {})
        if (display.get("space") != "cg-top-left-points" or display.get("main") is not True
                or display.get("single") is not True):
            raise AdmissionError("Placement display admission unavailable")
        display_geometry(display)
        scale = display["scale"]
        expected = dict(X=self.origin[0], Y=self.origin[1], Width=SIZE[0], Height=SIZE[1])
        if (metadata.get("kCGWindowBounds") != expected
                or metadata.get("capturePixelSize") not in ([SIZE[0] * scale, SIZE[1] * scale], (SIZE[0] * scale, SIZE[1] * scale))
                or metadata.get("capturePixelsPerWindowPoint") != scale
                or metadata.get("kCGWindowOwnerPID") != self.window.process.pid
                or type(metadata.get("kCGWindowNumber")) is not int):
            raise AdmissionError("Placement capture does not match owned geometry")
        if self.display is not None and self.display != display:
            raise AdmissionError("Placement display changed")
        if self.window_id is not None and self.window_id != metadata["kCGWindowNumber"]:
            raise AdmissionError("Placement owned window changed")
        self.display, self.window_id = display, metadata["kCGWindowNumber"]

    def target(self, kind, label, point, bounds):
        if self.window_id is None or not self.assigned or len(self.targets) >= 256:
            raise AdmissionError("Placement target has no admitted capture")
        x, y = point
        # OCR bounds use capture pixels; targets and window bounds use points.
        scale = self.display["scale"]
        box = dict(x=bounds["left"] / scale, y=bounds["top"] / scale,
                   width=bounds["width"] / scale, height=bounds["height"] / scale)
        rectangle(box)
        window = dict(x=0, y=0, width=SIZE[0], height=SIZE[1])
        if (not contains(window, box) or not all(math.isfinite(v) for v in point)
                or not 0 <= x < SIZE[0] or not 0 <= y < SIZE[1]
                or kind in ("nav", "content") and not (box["x"] <= x <= box["x"] + box["width"]
                    and box["y"] <= y <= box["y"] + box["height"])):
            raise AdmissionError("Placement target escapes measured control/window")
        self.targets.append(dict(kind=kind, label=label, point=point, bounds=bounds))

    def cleaned(self, processes, observer, errors):
        safe = not errors and all(p is None or p.poll() is not None for p in processes)
        try:
            if observer is not None:
                safe = safe and observer.finished.is_set()
                self.healthy = self.healthy and observer.reason is None and observer.receipt().get("stopped") is True
            else:
                self.healthy = False
        except Exception:
            safe, self.healthy = False, False
        self.cleanup.append(bool(safe))

    def receipt(self):
        return dict(origin=list(self.origin), assigned=self.assigned, display=self.display,
                    targets=self.targets, cleanupSafe=bool(self.cleanup) and all(self.cleanup),
                    observerHealthy=self.healthy)


def backend_quiescent(backend):
    try:
        return (backend.child is not None and backend.child.poll() is not None
                and not backend.owns_local_state and not backend.generated.exists()
                and (backend.log is None or backend.log.closed))
    except Exception:
        return False


def write_receipt(output, value):
    target = output / "placement-comparison.json"
    if target.is_symlink():
        raise AdmissionError("Placement receipt link refused")
    temporary = output / "placement-comparison.pending"
    with temporary.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(target)


def persist(output, value, primary):
    try:
        write_receipt(output, value)
    except Exception:
        if primary is not None:
            primary.add_note("Placement evidence write failed")
            raise primary
        raise


def compare(arms):
    if len(arms) != 2:
        return "inconclusive"
    a, b = arms
    ar, br = a["geometry"], b["geometry"]
    matched = (ar["assigned"] and br["assigned"] and ar["display"] == br["display"]
               and ar["targets"] == br["targets"][:len(ar["targets"])]
               and bool(ar["targets"]) and all(v["cleanupSafe"] and v["observerHealthy"] for v in (ar, br)))
    if not matched or a["endpoint"] != "sound-on":
        return "inconclusive"
    if b["status"] == "passed":
        return "placement-dependent-in-this-pair"
    return "not-supported-for-this-shift" if b["endpoint"] == "sound-on" else "inconclusive"


def run_pair(executable, output, checkout, package, qualify, backend_type):
    guard()
    output = owned_path(output, exists=False)
    if output.exists():
        raise AdmissionError("Placement output must be fresh")
    identity = provenance(executable, package, checkout)
    initial_display = display_receipt()
    output.mkdir()
    deadline = time.monotonic() + 1200  # Outer existing ARM scene remains the hard total bound.
    arms, first = [], None
    receipt = dict(mode=MODE, provenance=identity, display=initial_display, arms=arms, result="incomplete",
                   coverageQualified=False, cutoverQualified=False)
    persist(output, receipt, first)
    for name, origin in zip(("A", "B"), ORIGINS):
        if time.monotonic() >= deadline:
            if first is not None:
                first.add_note("Placement shared scene deadline expired before next arm")
                raise first
            raise TimeoutError("Placement shared scene deadline expired")
        try:
            unchanged = provenance(executable, package, checkout) == identity and display_receipt() == initial_display
        except Exception:
            if first is not None:
                first.add_note("Placement admission failed; next arm refused")
                raise first
            raise
        if not unchanged:
            if first is not None:
                first.add_note("Placement provenance changed; next arm refused")
                raise first
            raise AdmissionError("Placement provenance changed")
        arm = Arm(origin, executable)
        arm.display = initial_display
        value = dict(arm=name, status="started", endpoint=None, geometry=arm.receipt())
        arms.append(value)
        persist(output, receipt, first)
        backend, failure = None, None
        try:
            backend = backend_type(checkout)
            with backend:
                qualify(executable, output / name, "macos", checkout, backend, placement=arm)
            value["status"] = "passed"
        except BaseException as error:
            failure = error
            if first is None:
                first = error
            value["status"] = "failed"
            value["endpoint"] = "sound-on" if type(error) is RuntimeError and str(error) == SOUND_FAILURE else "other"
        finally:
            value["geometry"] = arm.receipt()
            value["backendQuiescent"] = backend is not None and backend_quiescent(backend)
            receipt["result"] = compare(arms)
            persist(output, receipt, first)
        safe = (value["backendQuiescent"] and value["geometry"]["cleanupSafe"]
                and value["geometry"]["observerHealthy"]
                and not isinstance(failure, AdmissionError)
                and (failure is None or isinstance(failure, Exception))
                and (arm.window is None or arm.assigned))
        if not safe:
            if first is not None:
                first.add_note("Placement owned cleanup/admission unsafe; remaining arm refused")
                raise first
            raise AdmissionError("Placement owned cleanup/admission unsafe")
    receipt["result"] = compare(arms)
    persist(output, receipt, first)
    if first is not None:
        raise first
    return receipt
