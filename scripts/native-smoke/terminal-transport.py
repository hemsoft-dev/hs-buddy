"""Exercise production terminal transport on an owned ephemeral CI runner, without desktop UI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


CHECKS = ("ansiColor", "unicode", "cwd", "unicodeExecutable", "actualPtyResize", "interrupt",
          "ownedChildExited", "unrelatedSentinelAlive", "inputWriteAcknowledgements")


def runner_root():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("Terminal CI transport qualification requires an ephemeral GitHub runner")
    raw = os.environ.get("RUNNER_TEMP", "")
    root = Path(raw)
    if not raw or not root.is_absolute() or not root.is_dir() or root.is_symlink():
        raise RuntimeError("Terminal CI transport requires an absolute owned RUNNER_TEMP directory")
    return root.resolve()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    root = runner_root()
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected-source", required=True)
    parser.add_argument("--host", type=Path)
    parser.add_argument("--host-sha256")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=all"], cwd=repo, text=True).strip()
    if head != args.expected_source or dirty:
        raise RuntimeError("Terminal qualification requires the exact clean source revision")
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    if not target.is_absolute():
        target = repo / "rust" / target
    suffix = ".exe" if os.name == "nt" else ""
    driver = target / "debug" / "examples" / f"terminal-smoke{suffix}"
    host = args.host or target / "debug" / "examples" / f"terminal-host{suffix}"
    for binary in (driver, host):
        if not binary.is_file() or binary.is_symlink():
            raise RuntimeError("Terminal transport executable is missing or a symbolic link")
    host_hash = digest(host)
    if args.host and (not args.host_sha256 or host_hash != args.host_sha256):
        raise RuntimeError("Installed terminal host hash does not match its qualification receipt")
    if not args.host and args.host_sha256:
        raise RuntimeError("A host hash requires an explicit installed host path")
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    evidence = root / "native-evidence" / "terminal-transport" / ("installed" if args.host else "core-example")
    evidence.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="buddy terminal 界 ", dir=root) as workspace:
        result = subprocess.run([str(driver), str(host), workspace], cwd=repo,
                                capture_output=True, text=True, encoding="utf-8", timeout=180,
                                creationflags=flags)
        (evidence / "stdout.log").write_text(result.stdout, encoding="utf-8")
        (evidence / "stderr.log").write_text(result.stderr, encoding="utf-8")
        if result.returncode:
            raise RuntimeError(f"Terminal transport failed with exit {result.returncode}; inspect its retained synthetic output")
        value = json.loads(result.stdout)
        if value.get("result") != "passed" or any(value.get(check) is not True for check in CHECKS):
            raise RuntimeError("Terminal transport did not acknowledge every required behavior check")
    receipt = dict(result="passed", head=head, sourceDirty=False,
                   version=json.loads((repo / "package.json").read_text(encoding="utf-8-sig"))["version"],
                   driverSha256=digest(driver), hostSha256=host_hash,
                   hostKind="installed" if args.host else "core-example",
                   desktopUi=False, checks=value)
    (evidence / "qualification.json").write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
