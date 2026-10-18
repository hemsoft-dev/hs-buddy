"""Own the passive CI monitor without replacing an application failure."""
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from desktop import finish_cleanup


def _capture_dialogs(rows, directory):
    """Capture only the system windows already proven to intercept this app."""
    identifiers = sorted({window["windowNumber"] for row in rows
        for window in row.get("referencedWindows", [])
        if window.get("available") is True
        and window.get("kCGWindowOwnerName") == "UserNotificationCenter"
        and window.get("kCGWindowOwnerPID") != row.get("ownedPid")
        and type(window.get("windowNumber")) is int and window["windowNumber"] > 0})
    if not identifiers:
        return
    if (sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1"):
        raise RuntimeError("System-dialog capture requires the disposable CI Mac fixture")
    for identifier in identifiers:
        target = directory / f"foreign-system-dialog-{identifier}.png"
        if target.exists():
            raise RuntimeError("System-dialog diagnostics must not reuse earlier evidence")
        subprocess.run(["screencapture", "-x", "-o", "-l", str(identifier), str(target)],
            check=True, capture_output=True, timeout=10)
        if not target.is_file() or target.stat().st_size == 0:
            raise RuntimeError("The intercepted system dialog did not produce a window capture")


@contextmanager
def monitor(process, directory):
    executable = os.environ.get("BUDDY_MAC_MOUSE_MONITOR")
    if not executable:
        yield
        return
    if (sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1"
            or process.pid <= 0 or process.poll() is not None):
        raise RuntimeError("Passive mouse diagnostics require the live owned CI Mac app")
    with _owned_monitor(executable, process.pid, directory):
        yield


@contextmanager
def _owned_monitor(executable, pid, directory):
    ready = directory / "native-mouse-ready.json"
    events = directory / "native-mouse-events.jsonl"
    if ready.exists() or events.exists():
        raise RuntimeError("Native mouse diagnostics must not reuse earlier evidence")
    child = None
    primary = None
    with (directory / "native-mouse-monitor.log").open("wb") as log:
        try:
            child = subprocess.Popen([str(executable), str(pid), str(directory)], stdout=log, stderr=log)
            deadline = time.monotonic() + 10
            while not ready.exists():
                if child.poll() is not None:
                    raise RuntimeError("The passive owned-process monitor exited before listening")
                if time.monotonic() >= deadline:
                    raise RuntimeError("The passive owned-process monitor did not become ready")
                time.sleep(0.05)
            receipt = json.loads(ready.read_text(encoding="utf-8"))
            if receipt != dict(result="listening", ownedPid=pid, listenOnly=True, mouseOnly=True):
                raise RuntimeError("The passive monitor did not confirm the exact owned process")
            yield
            if child.poll() is not None:
                raise RuntimeError("The passive owned-process monitor exited during the exercise")
        except BaseException as failure:
            primary = failure
            raise
        finally:
            if child is not None:
                def cleanup():
                    if child.poll() is None:
                        child.terminate()
                    code = child.wait(timeout=10)
                    if code not in (0, -15):
                        raise RuntimeError("The passive owned-process monitor failed")
                    rows = [json.loads(line) for line in events.read_text(encoding="utf-8").splitlines()]
                    targets = directory / "targets.jsonl"
                    had_mouse_actions = targets.exists() and bool(targets.read_text(encoding="utf-8").strip())
                    if ((had_mouse_actions and not rows)
                            or any(row.get("ownedPid") != pid or row.get("tapDisabled") for row in rows)):
                        raise RuntimeError("The passive monitor did not preserve valid owned-process events")
                    _capture_dialogs(rows, directory)
                finish_cleanup(cleanup, primary, "Passive mouse diagnostic cleanup")
