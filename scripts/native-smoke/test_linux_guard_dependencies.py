"""Keep pre-install failure controls independent of optional runtime packages."""
from pathlib import Path
import subprocess
import sys
import unittest


class GuardDependencyControls(unittest.TestCase):
    def test_crew_controls_run_with_all_site_packages_disabled(self):
        here = Path(__file__).resolve().parent
        result = subprocess.run([sys.executable, "-I", "-S", "-m", "unittest", "discover",
            "-s", str(here), "-p", "test_linux_lifecycle_crew_capture.py", "-v"],
            cwd=here.parents[1], capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
