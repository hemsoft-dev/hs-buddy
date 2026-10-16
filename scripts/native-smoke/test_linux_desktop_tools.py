"""Exercise the actual shared apt invocation without installing packages or UI."""
import os
from pathlib import Path
import shutil
import subprocess
import unittest


class LinuxDesktopTools(unittest.TestCase):
    def test_container_and_host_privilege_prefixes_receive_the_complete_tool_set(self):
        bash = shutil.which("bash")
        if os.name == "nt":
            git_bash = Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe"
            if git_bash.is_file():
                bash = str(git_bash)
        self.assertIsNotNone(bash, "Bash is required to validate the native qualification scripts")
        helper = Path(__file__).with_name("linux-desktop-tools.sh").read_text(encoding="utf-8")
        # Shell functions intercept every command. No apt, privilege change,
        # runner impersonation, display, clipboard or application is involved.
        capture = '\napt-get() { printf "%s\\n" "$@"; }\nsudo() { "$@"; }\n'
        for prefix in ("apt-get", "sudo apt-get"):
            with self.subTest(prefix=prefix):
                result = subprocess.run([bash, "--noprofile", "--norc", "-s"],
                    input=helper + capture + f"buddy_install_linux_desktop_tools {prefix}\n",
                    text=True, capture_output=True, check=True, timeout=15)
                arguments = result.stdout.splitlines()
                self.assertEqual(arguments[:3], ["install", "-y", "--no-install-recommends"])
                self.assertIn("xclip", arguments)
                self.assertIn("fonts-noto-cjk", arguments)
                self.assertEqual(len(arguments), len(set(arguments)))


if __name__ == "__main__":
    unittest.main()
