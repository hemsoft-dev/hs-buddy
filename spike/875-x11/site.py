"""The browser qualification's fixture pages, with the probe split by feature.

/probe?only=popup,geo,camera,notification,iframe,private,download serves the
probe with just those attempts (no `only` = every attempt, as in
scripts/native-smoke/browser-desktop.py). /ok counts visits in a cookie.
usage: site.py <port>
"""
import http.server
import sys
from urllib.parse import parse_qs, urlparse

PARTS = {
    "iframe": '<iframe src="http://127.0.0.3:PORT/frame" width="240" height="40"></iframe>',
    "popup": "try { window.open('/ok', '_blank'); } catch (e) {}",
    "geo": "navigator.geolocation && navigator.geolocation.getCurrentPosition(() => 0, () => 0, {timeout: 3000});",
    "camera": "navigator.mediaDevices && navigator.mediaDevices.getUserMedia({video: true}).then(() => 0, () => 0);",
    "notification": "window.Notification && Promise.resolve(Notification.requestPermission()).then(() => 0, () => 0);",
    "private": "setTimeout(() => { location.href = 'http://127.0.0.2:PORT/private'; }, 1500);",
    "download": "setTimeout(() => { location.href = '/download'; }, 3500);",
}


def probe(port, only):
    chosen = [name for name in PARTS if not only or name in only]
    frame = PARTS["iframe"] if "iframe" in chosen else ""
    script = "\n".join(PARTS[name] for name in chosen if name != "iframe")
    page = ("<!doctype html><html><head><title>Buddy Probe</title></head>"
            "<body style='font:40px sans-serif;margin:32px'><p>PROBE PAGE</p>"
            f"<p>{' '.join(chosen) or 'nothing'}</p>{frame}<script>{script}</script></body></html>")
    return page.replace("PORT", str(port))


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send(self, status, body, content_type="text/html; charset=utf-8", headers=()):
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/ok":
            cookie = self.headers.get("Cookie", "")
            visits = 1 + sum(int(part.split("=", 1)[1]) for part in cookie.split("; ")
                if part.startswith("visits=") and part.split("=", 1)[1].isdigit())
            self.send(200, "<!doctype html><html><head><title>Buddy Fixture OK</title></head>"
                "<body style='font:48px sans-serif;margin:32px'><p>FIXTURE PAGE</p>"
                f"<p>Visit {visits}</p></body></html>",
                headers=[("Set-Cookie", f"visits={visits}; Max-Age=86400; Path=/; SameSite=Lax")])
        elif url.path == "/probe":
            only = set(",".join(parse_qs(url.query).get("only", [""])).split(",")) - {""}
            self.send(200, probe(self.server.server_port, only))
        elif url.path == "/download":
            self.send(200, "not for saving", "application/octet-stream",
                [("Content-Disposition", "attachment; filename=fixture.bin")])
        else:
            self.send(404, "missing")


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
