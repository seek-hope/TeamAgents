#!/usr/bin/env python3
"""Minimal HTTP health service using only the Python 3 standard library.

Serves GET /health on 127.0.0.1:8765 with the exact body "p6-service-ok".
Listens only on the loopback interface.
"""
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = 8765
BODY = b"p6-service-ok"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send(self, code: int, body: bytes, ctype: str = "text/plain; charset=utf-8") -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802 (stdlib naming)
        if self.path == "/health":
            self._send(200, BODY)
        else:
            self._send(404, b"not found")

    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))
        sys.stderr.flush()


def main() -> None:
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    sys.stderr.write("listening on http://%s:%d/health\n" % (HOST, PORT))
    sys.stderr.flush()
    server.serve_forever()


if __name__ == "__main__":
    main()
