"""Exercise the actual shared apt invocation without installing packages or UI."""
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import unittest

OPTIONS = ["-o", "Acquire::Retries=3", "-o", "Acquire::http::Timeout=30", "-o", "Acquire::https::Timeout=30"]
# Shell functions intercept every command. No apt, dpkg, privilege change,
# runner impersonation, display, clipboard or application is involved.
CAPTURE = """
timeout() { printf 'timeout %s %s\\n' "$1" "$2" >&2; shift 2; "$@"; }
dpkg() { printf 'dpkg %s\\n' "$*" >&2; }
sudo() { printf 'sudo\\n' >&2; "$@"; }
apt-get() {
  if [[ "$1" == indextargets ]]; then
    printf '%s\\n' http://azure.archive.ubuntu.com/ubuntu/ http://azure.archive.ubuntu.com/ubuntu/
    return
  fi
  printf '%s\\n' "$@"
  return "${APT_STATUS:-0}"
}
"""


def bash():
    executable = shutil.which("bash")
    if os.name == "nt":
        git_bash = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe"
        if git_bash.is_file():
            executable = str(git_bash)
    return executable


def run(command, status=0):
    executable = bash()
    if executable is None:
        raise AssertionError("Bash is required to validate the native qualification scripts")
    helper = Path(__file__).with_name("linux-desktop-tools.sh").as_posix()
    return subprocess.run([executable, "--noprofile", "--norc", "-s"],
        input=f"set -euo pipefail\nsource {shlex.quote(helper)}\n{CAPTURE}{command}\n",
        env={**os.environ, "APT_STATUS": str(status), "GITHUB_ACTIONS": "true"},
        text=True, capture_output=True, timeout=15)


class LinuxDesktopTools(unittest.TestCase):
    def test_container_and_host_privilege_prefixes_receive_the_complete_tool_set(self):
        for prefix, privileged in (("", False), ("sudo", True)):
            with self.subTest(prefix=prefix):
                result = run(f"buddy_install_linux_desktop_tools {prefix}")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual("sudo" in result.stderr.splitlines(), privileged)
                self.assertIn("timeout --kill-after=30 600", result.stderr)
                arguments = result.stdout.splitlines()
                self.assertEqual(arguments[:len(OPTIONS)], OPTIONS)
                tools = arguments[len(OPTIONS):]
                self.assertEqual(tools[:3], ["install", "-y", "--no-install-recommends"])
                self.assertIn("xclip", tools)
                self.assertIn("fonts-noto-cjk", tools)
                self.assertEqual(len(tools), len(set(tools)))

    def test_update_has_a_shorter_limit(self):
        result = run("buddy_apt update")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("timeout --kill-after=30 300", result.stderr)
        self.assertEqual(result.stdout.splitlines(), OPTIONS + ["update"])

    def test_exhausted_attempts_fail_naming_the_mirror_and_step(self):
        result = run("buddy_apt sudo install -y xvfb || echo failed=$?", status=124)
        self.assertEqual(result.stdout.splitlines()[-1], "failed=1")
        self.assertEqual(result.stdout.count("xvfb"), 2)
        self.assertEqual(result.stderr.count("dpkg --configure -a"), 1)
        message = result.stderr.splitlines()[-1]
        self.assertTrue(message.startswith("::error title=apt-get failed::"), message)
        self.assertIn("no result within 600s", message)
        self.assertIn("Step: apt-get install -y xvfb", message)
        self.assertIn("Mirrors: http://azure.archive.ubuntu.com/ubuntu/", message)
        self.assertEqual(message.count("azure"), 1)

    def test_failed_update_reports_its_exit_status(self):
        result = run("buddy_apt update || echo failed=$?", status=100)
        self.assertEqual(result.stdout.splitlines()[-1], "failed=1")
        self.assertNotIn("dpkg", result.stderr)
        self.assertIn("(exit 100)", result.stderr)


if __name__ == "__main__":
    unittest.main()
