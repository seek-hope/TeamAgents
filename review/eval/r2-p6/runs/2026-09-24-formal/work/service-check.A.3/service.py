#!/usr/bin/env python3
"""Minimal HTTP service exposing /health on 127.0.0.1:8765.

Standard library only. Response body for /health is exactly: p6-service-ok
"""
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = 8765
BODY = b"p6-service-ok"


class Handler(BaseHTTPRequestHandler):
    server_version = "P6Service/1.0"
    protocol_version = "HTTP/1.1"

    def _send(self, status, body, ctype="text/plain; charset=utf-8"):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def do_GET(self):
        if self.path.split("?", 1)[0] == "/health":
            self._send(200, BODY)
        else:
            self._send(404, b"not found")

    def do_HEAD(self):
        self.do_GET()

    def log_message(self, fmt, *args):
        sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))
        sys.stderr.flush()


def main():
    httpd = ThreadingHTTPServer((HOST, PORT), Handler)
    sys.stderr.write("listening on http://%s:%d (pid=%d)\n" % (HOST, PORT, __import__("os").getpid()))
    sys.stderr.flush()
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        httpd.server_close()


if __name__ == "__main__":
    main()
