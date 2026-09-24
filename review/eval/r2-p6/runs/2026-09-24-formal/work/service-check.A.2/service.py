#!/usr/bin/env python3
"""Minimal HTTP service exposing GET /health on 127.0.0.1:8765.

Uses only the Python 3 standard library. No third-party dependencies.
The response body for /health is exactly: p6-service-ok
"""

import argparse
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BODY = b"p6-service-ok"


class Handler(BaseHTTPRequestHandler):
    server_version = "p6-service/1.0"
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/health":
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(BODY)))
            self.end_headers()
            self.wfile.write(BODY)
        else:
            msg = b"not found"
            self.send_response(404)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(msg)))
            self.end_headers()
            self.wfile.write(msg)

    do_HEAD = do_GET

    def log_message(self, fmt, *args):  # keep stdout/stderr tidy but useful
        sys.stderr.write("service.py: %s - %s\n" % (self.address_string(), fmt % args))
        sys.stderr.flush()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()

    httpd = ThreadingHTTPServer((args.host, args.port), Handler)
    print("listening on http://%s:%d/health" % (args.host, args.port), flush=True)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        httpd.server_close()


if __name__ == "__main__":
    main()
