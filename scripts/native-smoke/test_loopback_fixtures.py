"""#913: no fixture HTTP server may open the macOS Local Network prompt.

`HTTPServer.server_bind` calls `socket.getfqdn`, even for 127.0.0.1. On the
hosted Macs that lookup shows "Allow Python to find devices on local networks?"
over the installed app, and the installed scenes' clicks then land on the
dialog. These suite-wide checks run on every platform, so a new plain server is
caught before it reaches a Mac job.
"""
import importlib.util
from pathlib import Path
import re
import socket
import unittest
import urllib.request
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
PLAIN_SERVER = re.compile(r"(?<![A-Za-z_])(?:Threading)?HTTPServer\s*\(")


def load(path):
    spec = importlib.util.spec_from_file_location(path.stem.replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class LoopbackFixtureTests(unittest.TestCase):
    def test_no_python_fixture_constructs_a_plain_http_server(self):
        offenders = []
        for directory in (HERE, ROOT / "perf"):
            for path in sorted(directory.glob("*.py")):
                if path.name in ("loopback_http.py", Path(__file__).name):
                    continue
                for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                    if PLAIN_SERVER.search(line):
                        offenders.append(f"{path.relative_to(ROOT).as_posix()}:{number}")
        self.assertEqual(offenders, [], "use loopback_http.LoopbackHTTPServer instead")

    def test_the_guard_recognizes_plain_servers_and_allows_the_loopback_one(self):
        self.assertIsNotNone(PLAIN_SERVER.search('server = ThreadingHTTPServer(("127.0.0.1", 0), Page)'))
        self.assertIsNotNone(PLAIN_SERVER.search("httpd = http.server.HTTPServer(address, Handler)"))
        self.assertIsNone(PLAIN_SERVER.search('server = LoopbackHTTPServer(("127.0.0.1", 0), Page)'))

    def test_lifecycle_browser_fixture_never_discovers_a_local_hostname(self):
        lifecycle = load(HERE / "linux-installed-lifecycle.py")
        refuse = AssertionError("The lifecycle fixture must not discover local devices")
        with patch.object(socket, "getfqdn", side_effect=refuse):
            with lifecycle.browser_fixture() as browser:
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
                with opener.open(browser["proxy"] + "/", timeout=5) as response:
                    self.assertIn(lifecycle.MARKER, response.read().decode())


if __name__ == "__main__":
    unittest.main()
