#!/usr/bin/env python3
"""Minimal HTTP service using only the Python 3 standard library.

Serves GET /health on 127.0.0.1:8765 with body exactly "p6-service-ok".
"""
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = 8765
HEALTH_BODY = b"p6-service-ok"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _send(self, status: int, body: bytes, ctype: str) -> None:
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_GET(self):  # noqa: N802 (stdlib naming)
        if self.path == "/health":
            self._send(200, HEALTH_BODY, "text/plain; charset=utf-8")
        else:
            self._send(404, b"not found", "text/plain; charset=utf-8")

    def do_HEAD(self):  # noqa: N802
        self.do_GET()

    def log_message(self, fmt, *args):  # keep logs on stderr, unbuffered-ish
        sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))
        sys.stderr.flush()


def main() -> None:
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    sys.stderr.write(f"listening on http://{HOST}:{PORT}\n")
    sys.stderr.flush()
    server.serve_forever()


if __name__ == "__main__":
    main()
