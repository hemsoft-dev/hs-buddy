"""Installed automation jobs, schedules and runs on disposable CI desktops and a local Convex backend."""
import argparse
import json
import os
from pathlib import Path
import shlex
import socket
import sys
import time
import types
import uuid

from desktop import fixture
import importlib.util

HERE = Path(__file__).resolve().parent
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
        raise RuntimeError("Automation desktop qualification requires a disposable GitHub Actions runner")


def owned_output(raw):
    require_runner()
    return terminal.owned_output(raw)


def shell():
    return "powershell" if os.name == "nt" else "bash"


def python_command(code):
    """A shell command running `code`; markers are assembled at run time."""
    if os.name == "nt":
        return "python -c '" + code.replace("'", "''") + "'"
    return "python3 -c " + shlex.quote(code)


def echo_command(proof, nonce):
    # The Unicode value arrives only through the job's explicit environment.
    return python_command(
        "import os,json;from pathlib import Path;"
        "print('NATIVE'+'AUTOMATION'+'READY',flush=True);"
        "print(os.environ['BUDDY_AUTOMATION_VALUE'],flush=True);"
        f"Path({str(proof)!r}).write_text(json.dumps(dict(run={nonce!r},cwd=os.getcwd(),"
        "value=os.environ['BUDDY_AUTOMATION_VALUE'])),encoding='utf-8')")


def long_command(children, nonce):
    return python_command(
        "import os,sys,json,subprocess,time;"
        "print('NATIVE'+'STARTED',flush=True);"
        "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(300)']);"
        f"open({str(children)!r},'a',encoding='utf-8').write(json.dumps(dict(run={nonce!r},pids=[os.getpid(),child.pid]))+'\\n');"
        "time.sleep(300)")


def job_arguments(name, command, workspace):
    return dict(name=name, workerType="exec", config=dict(command=command, shell=shell(), cwd=str(workspace),
        timeout=600000, env=dict(PYTHONIOENCODING="utf-8", BUDDY_AUTOMATION_VALUE=UNICODE)))


def verify_proof(path, nonce, workspace):
    value = json.loads(path.read_text(encoding="utf-8"))
    # macOS reports getcwd() through symlinks resolved (/var is /private/var).
    if (set(value) != {"run", "cwd", "value"} or value["run"] != nonce or value["value"] != UNICODE
            or not isinstance(value["cwd"], str) or Path(value["cwd"]).resolve() != Path(workspace).resolve()):
        raise RuntimeError("The job did not run once in its Unicode directory with its explicit environment")


def runs_for(backend, job):
    return [run for run in backend.query("runs:listRecent") if run.get("jobId") == job]


def single_run(backend, job, status):
    runs = runs_for(backend, job)
    if len(runs) != 1:
        raise RuntimeError(f"Expected exactly one run of the job, observed {len(runs)}")
    if runs[0].get("status") != status:
        raise RuntimeError(f"Expected a {status} run, observed {runs[0].get('status')}")
    return runs[0]


def verify_output(run):
    # Command workers store {stdout, stderr, exitCode, ...}, as Electron does.
    output = run.get("output")
    stdout = output.get("stdout") if isinstance(output, dict) else None
    if (not isinstance(stdout, str) or "NATIVEAUTOMATIONREADY" not in stdout or UNICODE not in stdout
            or output.get("exitCode") != 0):
        raise RuntimeError("The completed run did not store the job's exact Unicode output")


def verify_cancelled_children(path, sentinel, nonce):
    records = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
    current = [record for record in records if record.get("run") == nonce]
    if len(current) != 1:
        raise RuntimeError("Cancellation requires exactly one receipt from this run")
    pids = current[0]["pids"]
    if len(pids) != 2 or len(set(pids)) != 2 or any(type(pid) is not int or pid <= 0 for pid in pids):
        raise RuntimeError("Invalid owned child identity receipt")
    deadline = time.monotonic() + 10
    while any(terminal.pid_running(pid) for pid in pids):
        if time.monotonic() >= deadline:
            raise RuntimeError("A cancelled job's process survived")
        time.sleep(0.2)
    terminal.verify_sentinel(sentinel, "job cancellation")


def unreachable_url():
    """A loopback port with nothing listening, owned by this process until it returns."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{probe.getsockname()[1]}"


def qualify_offline(executable, output, session):
    driver = settings.Driver(executable, output, session, types.SimpleNamespace(url=unreachable_url()), HERE.parent.parent)
    offline = driver.drive("offline-dark", [("sleep", 6), ("capture", "offline-dark")])
    github.check(offline / "offline-dark.png", "Offline", "Automation is offline", "Buddy keeps trying",
        absent=("Echo job", "Long job"))


def only(items, message):
    if len(items) != 1:
        raise RuntimeError(message)
    return items[0]


def qualify_runs(driver, backend, output, workspace, sentinel):
    proof, children = output / "run-proof.json", output / "owned-children.jsonl"
    nonce, long_nonce = str(uuid.uuid4()), str(uuid.uuid4())
    echo = github.mutate(backend, "jobs:create", job_arguments("Echo job", echo_command(proof, nonce), workspace))
    long = github.mutate(backend, "jobs:create", job_arguments("Long job", long_command(children, long_nonce), workspace))
    runs = driver.drive("runs-dark", [
        ("input", "Filter", "Echo job"), ("sleep", 1), ("capture", "jobs-dark"),
        ("click", "Run now"), ("capture", "run-input-dark"), ("click", "Queue run"), ("sleep", 1),
        ("click", "History"), ("sleep", 1), ("click", "Show output"),
        ("wait_text", "Completed"), ("wait_text", "NATIVEAUTOMATIONREADY"), ("capture", "output-dark"),
        ("input", "Filter", "Long job"), ("click", "Jobs"), ("sleep", 1),
        ("click", "Run now"), ("click", "Queue run"), ("sleep", 1),
        ("click", "History"), ("sleep", 1), ("click", "Show output"),
        ("wait_text", "NATIVESTARTED"), ("capture", "running-dark"),
        ("click", "Cancel run"), ("wait_text", "Cancelled"), ("sleep", 2), ("capture", "cancelled-dark"),
    ], record=True)
    github.check(runs / "jobs-dark.png", "Echo job", "Run now", "Edit", "Duplicate", absent=("Long job",))
    github.check(runs / "run-input-dark.png", "Run Echo job now", "Queue run")
    github.check(runs / "output-dark.png", "Completed", "NATIVEAUTOMATIONREADY", "Manual")
    github.check(runs / "running-dark.png", "Running", "NATIVESTARTED", "Cancel run")
    github.check(runs / "cancelled-dark.png", "Cancelled")
    verify_proof(proof, nonce, workspace)
    verify_output(single_run(backend, echo, "completed"))
    single_run(backend, long, "cancelled")
    verify_cancelled_children(children, sentinel, long_nonce)
    return echo, long


def qualify_editing(driver, backend, config, echo, long):
    light = json.loads(fixture(config.parent / "light-template", "light").read_text())
    terminal.write_config(config, lambda value: value["ui"].update(light["ui"]))
    restarted = driver.drive("restart-light", [
        ("click", "History"), ("sleep", 2), ("capture", "history-after-restart-light"),
        ("click", "Jobs"), ("click", "New job"), ("sleep", 1), ("capture", "new-job-light"),
        ("input", "Name", "Native created job"), ("input", "Command", "echo native_created"),
        ("click", "Save"), ("sleep", 3),
        ("input", "Filter", "Native created"), ("sleep", 1), ("capture", "job-created-light"),
        ("click", "Schedule"), ("sleep", 1), ("input", "Name", "Native weekdays"),
        ("click", "Run the latest missed"), ("capture", "schedule-editor-light"), ("click", "Save"), ("sleep", 3),
        ("click", "Schedules"), ("sleep", 1), ("capture", "schedule-created-light"),
        ("click", "Pause"), ("sleep", 3), ("capture", "schedule-paused-light"),
    ])
    github.check(restarted / "history-after-restart-light.png", "Completed", "Cancelled", "Echo job", "Long job")
    github.check(restarted / "new-job-light.png", "New job", "Shell command", "Command", "Save")
    github.check(restarted / "job-created-light.png", "Native created job", "Run now")
    github.check(restarted / "schedule-editor-light.png", "New schedule", "Native created job", "Run the latest missed")
    github.check(restarted / "schedule-created-light.png", "Native weekdays", "Enabled", "Run the latest missed")
    github.check(restarted / "schedule-paused-light.png", "Native weekdays", "Paused", "Enable", "Not scheduled")
    # The restarted dispatcher reconciles; it never runs either job again.
    single_run(backend, echo, "completed")
    single_run(backend, long, "cancelled")
    created = only([job for job in backend.query("jobs:list") if job["name"] == "Native created job"],
        "The native job create did not reach the backend exactly once")
    if created["workerType"] != "exec" or created["config"] != dict(command="echo native_created", shell=shell()):
        raise RuntimeError("The native job create sent the wrong worker settings")
    schedule = only([item for item in backend.query("schedules:list") if item["name"] == "Native weekdays"],
        "The native schedule create did not reach the backend exactly once")
    if (schedule["jobId"] != created["_id"] or schedule["cron"] != "0 9 * * 1-5" or schedule.get("timezone") != "America/New_York"
            or schedule["missedPolicy"] != "last" or schedule["enabled"] is not False or schedule.get("nextRunAt") is not None):
        raise RuntimeError("The native schedule did not keep its job, cron, timezone, missed policy and paused state")

    # Echo and Long job commands contain NATIVE markers; filter on the full name.
    changed = driver.drive("change-light", [
        ("input", "Filter", "Native created"), ("click", "Schedules"), ("sleep", 1),
        ("click", "Delete"), ("capture", "schedule-delete-light"), ("click", "Delete schedule"), ("sleep", 3),
        ("click", "Jobs"), ("sleep", 1), ("click", "Edit"), ("sleep", 1), ("capture", "edit-job-light"),
        ("input", "Command", "echo native_updated"), ("click", "Save"), ("sleep", 3),
        ("resize", 900, 650), ("sleep", 1), ("capture", "compact-light"),
    ])
    github.check(changed / "schedule-delete-light.png", "Native weekdays will be removed.", "Delete schedule", "Keep")
    github.check(changed / "edit-job-light.png", "Edit job", "echo native_created")
    github.check(changed / "compact-light.png", "Native created job", "Run now")
    if any(item["name"] == "Native weekdays" for item in backend.query("schedules:list")):
        raise RuntimeError("The confirmed schedule delete did not reach the backend")
    updated = only([job for job in backend.query("jobs:list") if job["name"] == "Native created job"],
        "The native job edit created or lost a job")
    if updated["_id"] != created["_id"] or updated["config"] != dict(command="echo native_updated", shell=shell()):
        raise RuntimeError("The native job edit did not update the same job")
    deleted = driver.drive("delete-light", [
        ("input", "Filter", "Native created"), ("sleep", 1),
        ("click", "Delete"), ("capture", "job-delete-light"), ("click", "Delete job"), ("sleep", 3),
        ("capture", "job-deleted-light"),
    ])
    github.check(deleted / "job-delete-light.png", "and its schedules will be removed.", "Delete job", "Keep")
    github.check(deleted / "job-deleted-light.png", "No jobs match the filter.")
    if any(job["name"] == "Native created job" for job in backend.query("jobs:list")):
        raise RuntimeError("The confirmed job delete did not reach the backend")


def qualify(executable, output, session, version, backend):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")
    workspace = output / "workspace with spaces 資料"
    workspace.mkdir()
    terminal.write_config(config, lambda value: value["native"].update(activeSection="automation"))
    driver = settings.Driver(executable, output, session, backend, HERE.parent.parent)
    sentinel = terminal.start_sentinel()
    primary = None
    try:
        # Start offline first; the next launch reconnects to the real backend.
        qualify_offline(executable, output, session)
        echo, long = qualify_runs(driver, backend, output, workspace, sentinel)
        qualify_editing(driver, backend, config, echo, long)
        terminal.verify_sentinel(sentinel, "the full automation exercise")
    except BaseException as failure:
        primary = failure
        raise
    finally:
        terminal.stop_sentinel(sentinel, primary)
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True,
        dataSource="Installed dispatcher and an isolated anonymous Convex deployment",
        checks=["offline start and reconnect", "manual run queued once", "Unicode working directory and explicit environment", "exact stored output",
            "live output while running", "cancellation stops the job's process family", "unrelated process preserved",
            "restart reconciles without rerunning", "job create/edit/delete", "schedule create/pause/delete",
            "missed policy and timezone", "compact window", "light/dark"]), indent=2), encoding="utf-8")


def main():
    # Check the host before argparse, filesystem access, deployment, or any input.
    require_runner()
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    output = owned_output(args.output)
    executable = args.executable.resolve(strict=True)
    with settings.SettingsBackend(HERE.parent.parent) as backend:
        qualify(executable, output, args.session, args.version, backend)


if __name__ == "__main__":
    main()
