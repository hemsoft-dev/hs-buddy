"""Numeric loopback fixtures never discover devices or request network access."""
from http.server import ThreadingHTTPServer
from socketserver import TCPServer


class LoopbackHTTPServer(ThreadingHTTPServer):
    def server_bind(self):
        if self.server_address[0] != "127.0.0.1":
            raise ValueError("Native HTTP fixtures require numeric IPv4 loopback")
        # HTTPServer.server_bind calls getfqdn even for a numeric address.
        # On macOS that discovery opens a permission dialog over the CI app.
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]
