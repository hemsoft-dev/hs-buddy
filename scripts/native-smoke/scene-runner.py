"""Time and bound one installed-package scene, including its owned processes."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def run_scene(name, command, timeout, receipts):
    if os.name != "posix" or timeout <= 0:
        raise ValueError("A Unix process group and a positive scene budget are required")
    receipts.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()

    def record(state, **details):
        value = dict(scene=name, state=state, at=datetime.now(timezone.utc).isoformat(),
            elapsedSeconds=round(time.monotonic() - started, 3), **details)
        with receipts.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(value) + "\n")
        print("native-scene " + json.dumps(value), flush=True)

    record("started", timeoutSeconds=timeout)
    process = subprocess.Popen(command, start_new_session=True)
    try:
        code = process.wait(timeout=timeout)
    except (subprocess.TimeoutExpired, KeyboardInterrupt) as failure:
        # Kill the owned group, not just the Python driver. OCR, the app and
        # fixture backends must not survive a timed-out scene.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait()
        record("timed-out" if isinstance(failure, subprocess.TimeoutExpired) else "interrupted")
        return 124 if isinstance(failure, subprocess.TimeoutExpired) else 130
    record("passed" if code == 0 else "failed", exitCode=code)
    return code if code >= 0 else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--name", required=True)
    parser.add_argument("--timeout", type=float, required=True)
    parser.add_argument("--receipts", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("A scene command is required")
    raise SystemExit(run_scene(args.name, command, args.timeout, args.receipts))


if __name__ == "__main__":
    main()
