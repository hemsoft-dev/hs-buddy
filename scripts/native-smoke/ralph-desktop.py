"""Installed Ralph loops on disposable CI desktops: launch, progress, logs, stop, reopen, errors.

The bundled loop scripts are replaced by `fixtures/ralph-stub.ps1` through
BUDDY_RALPH_SCRIPTS_DIR, and `copilot` by a stub on PATH, so nothing calls a model or
changes a repository. The real catalog, validation, tool discovery, detached host and
process ownership are exercised unchanged.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time
import types

from desktop import fixture, run
import importlib.util

HERE = Path(__file__).resolve().parent
CHECKOUT = HERE.parent.parent
UNICODE = "資料界é"


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


terminal = load("terminal-desktop")
github = terminal.github
settings = terminal.settings


def require_runner():
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Ralph desktop qualification requires a disposable GitHub Actions runner")


def unreachable_url():
    """Ralph needs no backend; the rest of the app sees a closed loopback port."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{probe.getsockname()[1]}"


def stub_scripts(directory):
    """The real configuration catalog with the stub in place of every loop script."""
    (directory / "config").mkdir(parents=True)
    for name in ("models.json", "providers.json", "agents.json"):
        shutil.copyfile(CHECKOUT / "scripts" / "ralph-loops" / "config" / name, directory / "config" / name)
    for name in ("ralph.ps1", "ralph-pr.ps1", "ralph-issues.ps1"):
        shutil.copyfile(HERE / "fixtures" / "ralph-stub.ps1", directory / name)
    return directory


def stub_tools(directory):
    """A `copilot` that only reports a version."""
    directory.mkdir()
    if os.name == "nt":
        (directory / "copilot.cmd").write_text("@echo stub-copilot 1.0\r\n", encoding="ascii")
    else:
        path = directory / "copilot"
        path.write_text("#!/bin/sh\necho stub-copilot 1.0\n", encoding="ascii")
        path.chmod(0o755)
    return directory


def git(repository, *arguments):
    return run("git", "-C", repository, *arguments)


def repository(path):
    path.mkdir()
    git(path, "init", "-q")
    (path / "README.md").write_text("Qualification repository\n", encoding="utf-8")
    git(path, "add", "README.md")
    git(path, "-c", "user.name=Buddy Qualification", "-c", "user.email=qualification@example.invalid",
        "commit", "-q", "-m", "Initial")
    return git(path, "rev-parse", "HEAD")


def verify_repository(path, head):
    if git(path, "rev-parse", "HEAD") != head or git(path, "status", "--porcelain", "--ignored"):
        raise RuntimeError("A loop changed the repository")


def receipts(directory):
    return [json.loads(path.read_text(encoding="utf-8")) for path in sorted(directory.glob("start-*.json"))]


def verify_receipts(directory, repository_path, expected):
    """Each launch started its loop exactly once, in the repository, with Buddy's defaults."""
    found = receipts(directory)
    counts = {mode: sum(1 for receipt in found if receipt["mode"] == mode) for mode in expected}
    if counts != expected or len(found) != sum(expected.values()):
        raise RuntimeError(f"Expected loop starts {expected}, observed {counts} of {len(found)}")
    for receipt in found:
        # macOS reports the working directory with /var resolved to /private/var.
        if Path(receipt["cwd"]).resolve() != repository_path.resolve():
            raise RuntimeError("A loop did not run in its repository")
        if receipt["noAudio"] is not True or receipt["autopilot"] is not True or receipt["max"] != 3:
            raise RuntimeError("A loop did not receive the launch form's defaults")
    return found


def runs(state):
    """Run requests and statuses as the installed app recorded them."""
    root = state / "native-ralph" / "runs"
    found = []
    for folder in sorted(root.iterdir()) if root.exists() else []:
        request = json.loads((folder / "request.json").read_text(encoding="utf-8"))
        status_path = folder / "status.json"
        status = json.loads(status_path.read_text(encoding="utf-8")) if status_path.exists() else None
        found.append(dict(request=request, status=status, folder=folder))
    return sorted(found, key=lambda item: item["request"]["createdAt"])


def wait_for(condition, message, timeout=30):
    deadline = time.monotonic() + timeout
    while not condition():
        if time.monotonic() >= deadline:
            raise RuntimeError(message)
        time.sleep(0.25)


def verify_stopped(receipt, sentinel):
    pids = [receipt["pid"], receipt["child"]]
    if any(type(pid) is not int or pid <= 0 for pid in pids):
        raise RuntimeError("Invalid loop process receipt")
    wait_for(lambda: not any(terminal.pid_running(pid) for pid in pids), "A stopped loop's process survived", 15)
    terminal.verify_sentinel(sentinel, "stopping a loop")


def qualify(executable, output, session, version):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")
    terminal.write_config(config, lambda value: value["native"].update(activeSection="ralph"))
    repository_path = output / f"repo with spaces {UNICODE}"
    head = repository(repository_path)
    proofs = output / "starts"
    proofs.mkdir()
    tools = stub_tools(output / "stub-tools")
    original_path = os.environ["PATH"]
    os.environ.update(BUDDY_RALPH_SCRIPTS_DIR=str(stub_scripts(output / "stub-scripts")),
        BUDDY_RALPH_PROOF=str(proofs), PATH=str(tools) + os.pathsep + original_path)
    driver = settings.Driver(executable, output, session, types.SimpleNamespace(url=unreachable_url()), CHECKOUT)
    sentinel = terminal.start_sentinel()
    primary = None
    repo = str(repository_path)
    try:
        launched = driver.drive("launch-dark", [
            ("wait_text", "Start a loop"), ("capture", "dashboard-dark"),
            ("click", "New loop"), ("sleep", 1),
            ("input", "Repository", repo), ("input", "Prompt", "quick"), ("capture", "form-dark"),
            ("click", "Launch"), ("wait_text", "RALPH STUB TOOL stub-copilot 1.0"), ("capture", "running-dark"),
            ("sleep", 8), ("wait_text", "Completed"), ("capture", "completed-dark"),
            ("click", "All loops"), ("sleep", 2), ("capture", "recent-dark"),
        ], record=True)
        github.check(launched / "dashboard-dark.png", "Ralph Loops", "Start a loop", "Ralph Loop", "Ralph PR", "Ralph Issues")
        github.check(launched / "form-dark.png", "Ralph Loop", "Launch", "Repository", "Provider", "Model",
            "Work agent", "Review agents", "Work iterations", "Repeat cycles", "Announce with audio")
        github.check(launched / "running-dark.png", "Running", "Iterating", "RALPH STUB TOOL", "Stop loop")
        github.check(launched / "completed-dark.png", "Completed", "RALPH STUB COMPLETE", "Retry", "Exit code 0")
        github.check(launched / "recent-dark.png", "Recent (1)", "Completed", "Ralph Loop")
        verify_receipts(proofs, repository_path, dict(quick=1))

        stopped = driver.drive("stop-dark", [
            ("click", "New loop"), ("sleep", 1), ("input", "Prompt", "long"),
            ("click", "Launch"), ("wait_text", "RALPH STUB RUNNING"), ("capture", "long-running-dark"),
            ("click", "Stop loop"), ("capture", "stop-confirm-dark"),
            ("click", "Stop now"), ("wait_text", "[Buddy] Stopped."), ("capture", "stopped-dark"),
        ], record=True)
        github.check(stopped / "long-running-dark.png", "Running", "RALPH STUB RUNNING")
        github.check(stopped / "stop-confirm-dark.png", "Stop this loop?", "Stop now", "Keep running")
        github.check(stopped / "stopped-dark.png", "Stopped", "[Buddy] Stopped.")
        long_runs = [receipt for receipt in verify_receipts(proofs, repository_path, dict(quick=1, long=1))
            if receipt["mode"] == "long"]
        verify_stopped(long_runs[0], sentinel)

        light = json.loads(fixture(config.parent / "light-template", "light").read_text())
        terminal.write_config(config, lambda value: value["ui"].update(light["ui"]))
        detached = driver.drive("detach-light", [
            ("click", "New loop"), ("sleep", 1), ("input", "Prompt", "long"),
            ("click", "Launch"), ("wait_text", "RALPH STUB RUNNING"), ("capture", "detached-running-light"),
        ])
        github.check(detached / "detached-running-light.png", "Running", "RALPH STUB RUNNING")
        # Buddy has quit; its loop keeps running under its own host.
        found = verify_receipts(proofs, repository_path, dict(quick=1, long=2))
        survivor = [receipt for receipt in found if receipt["mode"] == "long" and receipt not in long_runs][0]
        if not all(terminal.pid_running(pid) for pid in (survivor["pid"], survivor["child"])):
            raise RuntimeError("The loop stopped when Buddy closed")
        reopened = driver.drive("reopen-light", [
            ("wait_text", "Active (1)"), ("capture", "reopened-light"),
            ("click", "Stop"), ("wait_text", "RALPH STUB RUNNING"), ("capture", "reopened-detail-light"),
            ("click", "Stop now"), ("wait_text", "[Buddy] Stopped."), ("capture", "reopened-stopped-light"),
        ], record=True)
        github.check(reopened / "reopened-light.png", "Active (1)", "Running", "Recent (2)")
        github.check(reopened / "reopened-detail-light.png", "Running", "RALPH STUB RUNNING", "Stop now")
        github.check(reopened / "reopened-stopped-light.png", "Stopped")
        # Reopening attached to the running loop; it never started a second one.
        verify_receipts(proofs, repository_path, dict(quick=1, long=2))
        verify_stopped(survivor, sentinel)

        not_repository = output / "plain folder"
        not_repository.mkdir()
        if any((folder / ".git").exists() for folder in (not_repository, *not_repository.parents)):
            raise RuntimeError("The evidence folder must be outside any Git repository")
        errors = driver.drive("errors-light", [
            ("click", "New loop"), ("sleep", 1), ("input", "Repository", str(not_repository)),
            ("click", "Launch"), ("wait_text", "is not inside a Git repository."), ("capture", "not-repository-light"),
            ("input", "Repository", repo), ("input", "Prompt", "fail"),
            ("click", "Launch"), ("wait_text", "Failed"), ("capture", "failed-light"),
            ("click", "Retry"), ("sleep", 6), ("wait_text", "Failed"), ("capture", "retried-light"),
            ("click", "All loops"), ("sleep", 1), ("resize", 900, 650), ("click", "New loop"), ("sleep", 1),
            ("capture", "compact-form-light"),
        ])
        github.check(errors / "not-repository-light.png", "is not inside a Git repository.", "Launch")
        github.check(errors / "failed-light.png", "Failed", "The loop exited with code 3.", "RALPH STUB FAILING", "Retry")
        github.check(errors / "retried-light.png", "Failed", "Started again as a new run.")
        github.check(errors / "compact-form-light.png", "Ralph Loop", "Launch", "Close", "Repository")
        verify_receipts(proofs, repository_path, dict(quick=1, long=2, fail=2))

        # The same launch without the Copilot CLI is refused with setup guidance.
        os.environ["PATH"] = original_path
        missing = driver.drive("missing-light", [
            ("click", "New loop"), ("sleep", 1), ("input", "Prompt", "quick"),
            ("click", "Launch"), ("wait_text", "GitHub Copilot CLI (copilot) was not found."),
            ("capture", "missing-tool-light"),
        ])
        github.check(missing / "missing-tool-light.png", "GitHub Copilot CLI (copilot) was not found.", "npm install")
        verify_receipts(proofs, repository_path, dict(quick=1, long=2, fail=2))

        recorded = runs(output)
        states = [item["status"]["state"] for item in recorded]
        if states != ["completed", "stopped", "stopped", "failed", "failed"]:
            raise RuntimeError(f"Unexpected recorded runs: {states}")
        if recorded[4]["request"]["retryOf"] != recorded[3]["request"]["id"]:
            raise RuntimeError("Retry did not record the run it repeats")
        log = (recorded[0]["folder"] / "output.log").read_text(encoding="utf-8")
        if f"RALPH STUB quick {UNICODE}" not in log or "[stderr]" in log:
            raise RuntimeError("The completed loop's log is not its exact Unicode output")
        verify_repository(repository_path, head)
        terminal.verify_sentinel(sentinel, "the full Ralph exercise")
    except BaseException as failure:
        primary = failure
        raise
    finally:
        os.environ["PATH"] = original_path
        stop_leftovers(output)
        terminal.stop_sentinel(sentinel, primary)
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True,
        dataSource="Installed app with stub loop scripts and a stub Copilot CLI",
        checks=["launch form", "running progress and log", "completed result", "stop ends the loop's processes only",
            "loop survives Buddy closing", "reopen shows the running loop without starting another",
            "repository validation", "failed exit code", "retry as a new run", "missing tool setup error",
            "repository unchanged", "compact window", "light/dark"]), indent=2), encoding="utf-8")


def stop_leftovers(output):
    """A failed check must not leave a loop running on the runner."""
    for item in runs(output) if (output / "native-ralph").exists() else []:
        if item["status"] is None or item["status"]["state"] == "running":
            (item["folder"] / "stop").write_text("stop", encoding="utf-8")


def main():
    # Check the host before argparse, filesystem access, or any input.
    require_runner()
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    output = terminal.owned_output(args.output)
    executable = args.executable.resolve(strict=True)
    qualify(executable, output, args.session, args.version)


if __name__ == "__main__":
    main()
