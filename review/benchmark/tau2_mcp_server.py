#!/usr/bin/env python3
"""MCP stdio server that exposes a τ³-bench domain's tools to TeamAgents (D-420).

This is the *product-facing* half of the bridge: TeamAgents binds it as an ordinary `[tools.*] kind = "mcp"`
service, so the tools arrive through the MCP path the product already has — the point of running a tool-use
benchmark against this product rather than writing another in-process LLM loop.

It holds **no** domain state. Every `tools/list` and every `tools/call` is forwarded over a Unix socket to the
process that owns the τ³-bench environment (`tau2_teamagents_agent.py`, running in the τ³-bench runner), which
answers with the schemas and with the environment's own `get_response` result. That is what keeps the tool calls
on the same database the simulated user and the grader work against: a bridge with its own copy of the state would
grade something that never happened.

Usage (the adapter writes it into the product's config): `tau2_mcp_server.py --socket <path>`
"""

from __future__ import annotations

import argparse
import json
import socket
import sys


def ask(socket_path: str, request: dict) -> dict:
    """One request/response round over the bridge. A fresh connection per call keeps the two sides independent of
    each other's lifetimes — the environment may be rebuilt between turns, this process may not."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(600)
        connection.connect(socket_path)
        connection.sendall((json.dumps(request) + "\n").encode())
        buffer = b""
        while not buffer.endswith(b"\n"):
            chunk = connection.recv(65536)
            if not chunk:
                break
            buffer += chunk
    if not buffer.strip():
        raise RuntimeError("the τ³-bench bridge closed without an answer")
    return json.loads(buffer.decode())


def send(message: dict) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--socket", required=True, help="the Unix socket the τ³-bench runner listens on")
    args = parser.parse_args()

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue
        method = request.get("method")
        request_id = request.get("id")
        if request_id is None:  # a notification (`notifications/initialized`) has no answer
            continue
        try:
            if method == "initialize":
                result = {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "tau3-bridge", "version": "1"},
                }
            elif method == "tools/list":
                catalogue = ask(args.socket, {"op": "list"})
                result = {"tools": catalogue["tools"]}
            elif method == "tools/call":
                params = request.get("params", {})
                answer = ask(
                    args.socket,
                    {"op": "call", "name": params.get("name", ""), "arguments": params.get("arguments") or {}},
                )
                result = {
                    "content": [{"type": "text", "text": answer.get("content", "")}],
                    "isError": bool(answer.get("isError")),
                }
            else:
                send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32601, "message": f"unknown {method}"}})
                continue
            send({"jsonrpc": "2.0", "id": request_id, "result": result})
        except Exception as error:  # an MCP error is a result the client can read, not a crash
            send({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32000, "message": str(error)}})
    return 0


if __name__ == "__main__":
    sys.exit(main())
