#!/usr/bin/env python3
"""Minimal HTTP service built on the Python standard library only.

Serves GET /health -> body exactly "p6-service-ok" on 127.0.0.1:8765.
"""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = 8765
BODY = b"p6-service-ok"


class HealthHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path.split("?", 1)[0] == "/health":
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(BODY)))
            self.end_headers()
            self.wfile.write(BODY)
        else:
            self.send_response(404)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", "9")
            self.end_headers()
            self.wfile.write(b"not found")

    def log_message(self, fmt, *args):
        # Log to stderr (captured in service.log) with a timestamp.
        super().log_message(fmt, *args)


if __name__ == "__main__":
    server = ThreadingHTTPServer((HOST, PORT), HealthHandler)
    print(f"serving on http://{HOST}:{PORT} (pid {__import__('os').getpid()})", flush=True)
    server.serve_forever()
