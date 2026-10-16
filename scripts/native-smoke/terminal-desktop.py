"""Installed PTYs and prompt tools on disposable CI desktops and a local Convex backend."""
import argparse
import ctypes
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
import uuid

from desktop import fixture
import importlib.util

HERE = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


github = load("github-desktop")
settings = github.settings


def owned_output(raw):
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Terminal desktop qualification requires a disposable GitHub Actions runner")
    root = Path(os.environ.get("RUNNER_TEMP", ""))
    if not root.is_absolute() or not root.is_dir() or root.is_symlink():
        raise RuntimeError("An absolute owned runner directory is required")
    output = Path(raw).resolve()
    if output.exists() or not output.is_relative_to(root.resolve()) or output == root.resolve():
        raise RuntimeError("A fresh terminal evidence directory within RUNNER_TEMP is required")
    return output


def platform_key():
    return "windows" if os.name == "nt" else "macos" if sys.platform == "darwin" else "linux"


def write_config(path, change):
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    change(value)
    path.write_text(json.dumps(value), encoding="utf-8")


def command_proof(output, nonce):
    proof, children = output / "command-proof.json", output / "owned-children.jsonl"
    # The markers are assembled by Python after Enter. The visible command line
    # cannot itself satisfy the screenshot's output assertions.
    code = ("import os,sys,json,subprocess,time,signal;from pathlib import Path;"
        "signal.signal(signal.SIGINT,lambda *_:(print('NATIVE'+'INTERRUPTED',flush=True),sys.exit(0)));"
        "print(chr(27)+'[31m'+'NATIVE'+'COLOR'+chr(27)+'[0m');"
        "print('NATIVE'+'READY');print('資料界é');"
        "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)']);"
        f"Path({str(proof)!r}).write_text(json.dumps(dict(run={nonce!r},cwd=os.getcwd(),unicode='資料界é')),encoding='utf-8');"
        f"open({str(children)!r},'a',encoding='utf-8').write(json.dumps(dict(run={nonce!r},pids=[os.getpid(),child.pid]))+'\\n');"
        "time.sleep(120)")
    if os.name == "nt":
        return "python -c '" + code.replace("'", "''") + "'"
    return "python3 -c " + shlex.quote(code)


def verify_proof(output, nonce, workspace):
    value = json.loads((output / "command-proof.json").read_text(encoding="utf-8"))
    if value != dict(run=nonce, cwd=str(workspace), unicode="資料界é"):
        raise RuntimeError("The current terminal command did not run in its exact Unicode working directory")


def pid_running(pid):
    if os.name == "nt":
        from ctypes import wintypes
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel.WaitForSingleObject.restype = wintypes.DWORD
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        handle = kernel.OpenProcess(0x00100000, False, pid)
        if not handle:
            if ctypes.get_last_error() == 87:
                return False
            raise RuntimeError("Could not verify the fixture child's process identity")
        try:
            result = kernel.WaitForSingleObject(handle, 0)
            if result not in (0, 258):
                raise RuntimeError("Could not observe fixture child completion")
            return result == 258
        finally:
            kernel.CloseHandle(handle)
    result = subprocess.run(["ps", "-p", str(pid), "-o", "stat="], capture_output=True, text=True, timeout=5)
    return bool(result.stdout.strip()) and not result.stdout.strip().startswith("Z")


def verify_children(output, sentinel, nonce):
    records = [json.loads(value) for value in (output / "owned-children.jsonl").read_text().splitlines()]
    current = [record for record in records if record.get("run") == nonce]
    if len(current) != 1:
        raise RuntimeError("Child completion requires exactly this command invocation's receipt")
    pids = current[0]["pids"]
    if any(type(pid) is not int or pid <= 0 for pid in pids):
        raise RuntimeError("Invalid owned child identity receipt")
    if not pids or len(set(pids)) != len(pids):
        raise RuntimeError("Expected distinct actual child identities from this fixture")
    deadline = time.monotonic() + 5
    while any(pid_running(pid) for pid in pids):
        if time.monotonic() >= deadline:
            raise RuntimeError("An owned terminal child survived app shutdown")
        time.sleep(0.1)
    if sentinel.poll() is not None:
        raise RuntimeError("The terminal app stopped an unrelated sentinel")


def stop_sentinel(sentinel, primary):
    try:
        if sentinel.poll() is None:
            sentinel.terminate()
        sentinel.wait(timeout=10)
    except BaseException as failure:
        if primary is None:
            raise
        primary.add_note(f"Owned sentinel cleanup also failed: {type(failure).__name__}")


def qualify(executable, output, session, version, backend):
    binary = github.installed_binary(executable, version)
    config = fixture(output, "dark")
    workspace = output / "workspace with spaces 資料"
    workspace.mkdir()
    write_config(config, lambda value: value["native"].update(activeSection="terminal"))
    github.mutate(backend, "terminalWorkspaces:saveWorkspace", dict(nodes=[dict(id="native-work", name="Native terminal",
        sortOrder=0, layout=dict(type="pane", id="native-pane", cwd=str(workspace)))], activeNodeId="native-work"))
    nonce = str(uuid.uuid4())
    prompt = github.mutate(backend, "terminalPrompts:create", dict(title="Qualification command", content=command_proof(output, nonce)))
    driver = settings.Driver(executable, output, session, backend, HERE.parent.parent)
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    sentinel = subprocess.Popen([sys.executable, "-c", "import time;time.sleep(240)"], creationflags=flags)
    primary = None
    try:
        dark = driver.drive("interactive-dark", [
            ("click", "Prompt library"), ("sleep", 1), ("click", "Insert + copy"),
            ("capture", "prompt-before-enter-dark"), ("key", "Return"), ("sleep", 1),
            ("capture", "unicode-color-dark"), ("click", "Hide prompts"), ("click", "Interrupt"),
            ("sleep", 1), ("capture", "interrupted-dark"), ("click", "Split right"),
            ("sleep", 2), ("click", "More space"), ("capture", "split-dark"), ("resize", 900, 650), ("capture", "compact-dark"),
        ], record=True)
        github.check(dark / "prompt-before-enter-dark.png", "Prompt library", absent=("NATIVEREADY",))
        github.check(dark / "unicode-color-dark.png", "NATIVEREADY", "NATIVECOLOR")
        github.check(dark / "interrupted-dark.png", "NATIVEINTERRUPTED")
        github.check(dark / "split-dark.png", "Shell 1", "Shell 2")
        github.check(dark / "compact-dark.png", "Split right", "Shell 1", "Shell 2")
        verify_proof(output, nonce, workspace)
        verify_children(output, sentinel, nonce)
        saved = backend.query("terminalWorkspaces:getWorkspace")
        layout = saved["nodes"][0]["layout"]
        if layout.get("type") != "split" or len(layout.get("children", [])) != 2 or any(child.get("cwd") != str(workspace) for child in layout["children"]):
            raise RuntimeError("Native split layout or Unicode directories were not persisted")
        if len(layout.get("sizes", [])) != 2 or abs(layout["sizes"][1] / sum(layout["sizes"]) - 0.6) > 0.01:
            raise RuntimeError("The native resize control did not persist the selected shell's share")
        nonce = str(uuid.uuid4())
        github.mutate(backend, "terminalPrompts:update", dict(id=prompt, content=command_proof(output, nonce)))
        management = driver.drive("workspace-management-dark", [
            ("click", "Prompt library"), ("sleep", 1), ("click", "Insert + copy"), ("key", "Return"),
            ("sleep", 1), ("click", "Hide prompts"),
            ("click", "New workspace"), ("sleep", 1), ("click", "Workspace details"),
            ("input", "Workspace name", "Scratch workspace"), ("click", "Rename"),
            ("click", "Hide workspace details"), ("capture", "new-workspace-dark"),
            ("click", "Native terminal"), ("capture", "switched-split-dark"),
            ("click", "Scratch workspace"), ("click", "Workspace details"), ("click", "Close workspace"),
            ("click", "Close workspace and children"), ("sleep", 1),
        ])
        github.check(management / "new-workspace-dark.png", "Scratch workspace", "Shell 1")
        github.check(management / "switched-split-dark.png", "Native terminal", "Shell 1", "Shell 2", "NATIVEREADY")
        verify_proof(output, nonce, workspace)
        verify_children(output, sentinel, nonce)
        retained = backend.query("terminalWorkspaces:getWorkspace")
        if len(retained["nodes"]) != 1 or retained["nodes"][0]["id"] != "native-work" or retained.get("activeNodeId") != "native-work":
            raise RuntimeError("Workspace creation, switching, or confirmed closure changed the wrong layout")

        light_template = output / "light-template"
        light = json.loads(fixture(light_template, "light").read_text())
        missing = str(output / "missing shell executable 界")
        def invalid(value):
            value["ui"].update(light["ui"])
            value["native"]["terminalShells"] = {platform_key(): dict(program=missing, arguments=[])}
        write_config(config, invalid)
        errors = driver.drive("shell-errors-light", [
            ("resize", 900, 650), ("sleep", 1), ("capture", "invalid-shell-light"),
            ("click", "Close shell"), ("click", "Close shell"), ("capture", "closed-shell-light"),
        ])
        github.check(errors / "invalid-shell-light.png", "Shell unavailable", "Restart shell")
        github.check(errors / "closed-shell-light.png", "Shell closed", "Restart shell")
        write_config(config, lambda value: value["native"]["terminalShells"].pop(platform_key()))
        nonce = str(uuid.uuid4())
        github.mutate(backend, "terminalPrompts:update", dict(id=prompt, content=command_proof(output, nonce)))
        restored = driver.drive("restart-light", [
            ("click", "Workspace details"), ("input", "Workspace name", "Native restored"), ("click", "Rename"),
            ("input", "Selected shell name", "Build shell"), ("click", "Rename shell"),
            ("input", "Working directory", str(workspace)), ("click", "Save directory"),
            ("click", "Hide workspace details"), ("click", "Restart shell"), ("sleep", 3),
            ("click", "Prompt library"), ("sleep", 1), ("click", "Insert + copy"), ("key", "Return"),
            ("sleep", 1), ("capture", "restarted-unicode-light"),
        ])
        github.check(restored / "restarted-unicode-light.png", "Native restored", "Build shell", "NATIVEREADY", "NATIVECOLOR")
        verify_proof(output, nonce, workspace)
        verify_children(output, sentinel, nonce)
        saved = backend.query("terminalWorkspaces:getWorkspace")
        if saved["nodes"][0]["name"] != "Native restored" or saved["nodes"][0]["layout"].get("name") != "Build shell":
            raise RuntimeError("Workspace and shell names did not reach the shared layout")
        github.mutate(backend, "terminalPrompts:remove", dict(id=prompt))
        qualify_prompt_crud(driver, backend)
        if sentinel.poll() is not None:
            raise RuntimeError("An unrelated sentinel did not survive the full terminal exercise")
    except BaseException as failure:
        primary = failure
        raise
    finally:
        stop_sentinel(sentinel, primary)
    (output / "qualification.json").write_text(json.dumps(dict(result="passed", version=version, session=session,
        **binary, largeWindow=driver.large_size, desktopUi=True,
        dataSource="Real installed PTYs and an isolated anonymous Convex deployment",
        checks=["prompt insertion waits for Enter", "Unicode and ANSI output", "Unicode working directory",
            "split resize and persisted layout", "compact window", "invalid shell", "closed shell", "explicit restart",
            "workspace and shell rename", "prompt create/update/delete", "app child cleanup", "unrelated process preserved",
            "workspace create/switch/close", "light/dark", "native Return key and interrupt"]), indent=2), encoding="utf-8")


def qualify_prompt_crud(driver, backend):
    created = driver.drive("prompt-create-light", [
        ("click", "Prompt library"), ("sleep", 1), ("click", "New prompt"),
        ("input", "Prompt name", "Native saved prompt"), ("input", "Prompt text", "echo native_saved_prompt"),
        ("click", "Save prompt"), ("sleep", 1), ("capture", "prompt-created-light"),
    ])
    github.check(created / "prompt-created-light.png", "Native saved prompt", "Insert + copy")
    prompts = backend.query("terminalPrompts:list")
    if len(prompts) != 1 or prompts[0]["content"] != "echo native_saved_prompt":
        raise RuntimeError("The native prompt create did not reach the backend")
    driver.drive("prompt-update-light", [
        ("click", "Prompt library"), ("sleep", 1), ("click", "Edit prompt"),
        ("input", "Prompt text", "echo native_updated_prompt"), ("click", "Save prompt"), ("sleep", 1),
    ])
    prompts = backend.query("terminalPrompts:list")
    if len(prompts) != 1 or prompts[0]["content"] != "echo native_updated_prompt" or prompts[0].get("revision") != 2:
        raise RuntimeError("The captured native prompt edit did not update exactly one revision")
    driver.drive("prompt-delete-light", [
        ("click", "Prompt library"), ("sleep", 1), ("click", "Edit prompt"), ("click", "Delete prompt"),
        ("click", "Delete saved prompt"), ("sleep", 1),
    ])
    if backend.query("terminalPrompts:list") != []:
        raise RuntimeError("The confirmed native prompt delete did not reach the backend")


def main():
    # Check the host before argparse, filesystem access, deployment, or any input.
    if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("BUDDY_SETTINGS_FIXTURE") != "1":
        raise RuntimeError("Terminal desktop qualification requires a disposable GitHub Actions runner")
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--session", choices=("windows", "macos", "x11"), required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    output = owned_output(args.output)
    executable = args.executable.resolve(strict=True)
    binary = github.installed_binary(executable, args.version)
    head = settings.run("git", "rev-parse", "HEAD", cwd=HERE.parent.parent)
    settings.run(sys.executable, HERE / "terminal-transport.py", "--expected-source", head,
        "--host", executable, "--host-sha256", binary["binarySha256"], cwd=HERE.parent.parent, timeout=180)
    with settings.SettingsBackend(HERE.parent.parent) as backend:
        qualify(executable, output, args.session, args.version, backend)


if __name__ == "__main__":
    main()
