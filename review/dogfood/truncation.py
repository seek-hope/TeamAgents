#!/usr/bin/env python3
"""A19's whole path, hermetically: a truncated stream through the real wire stack (D-123).

A19 has two halves already: the provider edge classifies a truncated stream per protocol (in-process fake
servers in `providers_fake`), and since D-117 the driver's retry is tested with a scripted provider. What
neither shows is the whole path over a real socket with the engine's own HTTP/SSE stack — the classification,
the retry inside the same turn, and what ends up in the conversation.

This probe stands up a local chat-completions server, so it needs **no credentials and no network**, and runs
two scenarios with the real binary:

* **truncated before any visible text** — the connection closes after a delta that carries only a tool-call id:
  the attempt is transient, the driver retries inside the turn, the second response completes with a `finish`
  call, and the goal settles `SUCCEEDED`. The session shows two attempts for one request: `FAILED` with
  `error_class` `Transient` (no usage — the lost attempt's cost is unknown, §6.3) and `COMPLETE`.
* **truncated after visible text** — the connection closes after a delta with content: the attempt is
  *permanent* (retrying could duplicate text the user already saw), the run fails with `permanent model
  error: …`, and the server sees exactly one request. The partial text is not in the conversation.

The probe's own request counter and the session database decide; no model and nothing on the wire outside
loopback is involved.

    python3 review/dogfood/truncation.py
    python3 review/dogfood/truncation.py --state-dir /tmp/ta-truncation
"""
import argparse
import atexit
import json
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_TRUNCATION_PROBE_KEY"

HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# Only a tool-call id, no arguments and no content: nothing visible reached the user, so the attempt may be
# retried (§7, `providers_fake::truncated_stream_before_output_is_transient`).
TRUNCATED_INVISIBLE = HEAD + b'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1"}]}}]}\n\n'
# Visible content followed by a closed connection: the same wire event, classified permanent.
TRUNCATED_VISIBLE = HEAD + b'data: {"choices":[{"delta":{"content":"partial answer"}}]}\n\n'
FINISH_ARGUMENTS = json.dumps({"status": "success", "summary": "recovered from a truncated stream"})
COMPLETE = HEAD + (
    'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"finish-1","function":'
    '{"name":"finish","arguments":' + json.dumps(FINISH_ARGUMENTS) + '}}]}}]}\n\n'
    'data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":7,'
    '"completion_tokens":3,"total_tokens":10}}\n\n'
    'data: [DONE]\n\n'
).encode()


class FakeChat(BaseHTTPRequestHandler):
    """One scripted response per POST; the script is replaced per scenario."""

    protocol_version = "HTTP/1.0"
    script: list[bytes] = []
    seen: list[dict] = []

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            self.seen.append(json.loads(body.decode("utf-8", "replace")))
        except json.JSONDecodeError:
            self.seen.append({"unparseable": True})
        reply = self.script.pop(0) if self.script else b"HTTP/1.1 500 Script exhausted\r\ncontent-length: 0\r\n\r\n"
        self.wfile.write(reply)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int) -> str:
    return f"""# Truncation probe (A19): a local chat-completions server, no credentials, no network.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "truncation-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = 2
"""


def session(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    try:
        return {
            "attempts": list(db.execute("SELECT attempt_id, status, usage_json, error_class FROM attempts ORDER BY rowid"))
            if _has_column(db, "attempts", "error_class")
            else list(db.execute("SELECT attempt_id, status, usage_json FROM attempts ORDER BY rowid")),
            "requests": list(db.execute("SELECT request_id, status FROM model_requests")),
            "goals": list(db.execute("SELECT id, status FROM goals")),
            "entries": list(db.execute("SELECT kind, message_json FROM context_entries ORDER BY epoch, idx")),
        }
    finally:
        db.close()


def _has_column(db, table: str, column: str) -> bool:
    return any(row[1] == column for row in db.execute(f"PRAGMA table_info({table})"))


def scenario(name: str, script: list[bytes], prompt: str, root: pathlib.Path) -> tuple[dict, dict, list]:
    """Run one scenario; returns (report, session facts, requests the server saw)."""
    FakeChat.script = list(script)
    FakeChat.seen = []
    state_root = root / "root"
    atexit.register(stop_daemon, state_root)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(config(PORT))
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"), "PATH": "/usr/bin:/bin",
           KEY_VAR: "not-a-key", "HOME": str(root)}
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "60",
         "--cwd", str(workspace), prompt],
        capture_output=True, text=True, env=env, timeout=180,
    )
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    return report, {"exit": run.returncode, "stderr": run.stderr.strip()[:300], **session(state_root)}, list(FakeChat.seen)


PORT = 0



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    global PORT
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-truncation)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    PORT = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = pathlib.Path(args.state_dir or "/tmp/ta-truncation")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    failures: list[str] = []
    try:
        # --- scenario 1: truncated before any visible text → retried inside the turn -------------
        report, facts, seen = scenario("invisible", [TRUNCATED_INVISIBLE, COMPLETE], "Do the work.", root / "invisible")
        print(f"1. invisible truncation: exit={facts['exit']} end={report.get('end')} "
              f"goal={report.get('goal_status')} requests_seen={len(seen)}")
        if facts["exit"] != 0 or report.get("goal_status") != "SUCCEEDED":
            failures.append(f"the retry did not carry the turn: exit={facts['exit']} {report}")
        if len(seen) != 2:
            failures.append(f"the driver sent {len(seen)} requests (one truncated + one retry expected)")
        attempts = facts["attempts"]
        print(f"   attempts: {attempts}")
        if len(attempts) != 2:
            failures.append(f"expected two attempts for the one request, saw {attempts}")
        else:
            first, second = attempts[0], attempts[1]
            if first[1] != "FAILED" or (len(first) > 3 and first[3] != "Transient"):
                failures.append(f"the truncated attempt is not recorded as a transient failure: {first}")
            if first[2] not in ("null", None) and first[2] not in (None, "null"):
                failures.append(f"the lost attempt should carry no usage: {first}")
            if second[1] != "COMPLETE" or second[2] in ("null", None):
                failures.append(f"the retry should be the complete, priced attempt: {second}")
        if len(facts["requests"]) != 1:
            failures.append(f"one request, {len(facts['requests'])}: {facts['requests']}")
        if any("c1" in entry for _, entry in facts["entries"]):
            failures.append("the truncated attempt's tool call reached the conversation")

        # --- scenario 2: truncated after visible text → permanent, no retry ----------------------
        report, facts, seen = scenario("visible", [TRUNCATED_VISIBLE], "Do the work.", root / "visible")
        print(f"2. visible truncation: exit={facts['exit']} end={report.get('end')} "
              f"failure={str(report.get('failure'))[:70]!r} requests_seen={len(seen)}")
        if facts["exit"] != 1:
            failures.append(f"a permanent failure must end the run with 1, got {facts['exit']}")
        if not str(report.get("failure") or "").startswith("permanent model error:"):
            failures.append(f"the failure is not classified permanent: {report.get('failure')!r}")
        if len(seen) != 1:
            failures.append(f"a permanent failure must not be retried: {len(seen)} requests")
        attempts = facts["attempts"]
        print(f"   attempts: {attempts}")
        if len(attempts) != 1 or (len(attempts[0]) > 3 and attempts[0][3] != "Permanent"):
            failures.append(f"the visible truncation is not recorded as a permanent failure: {attempts}")
        if any("partial answer" in entry for _, entry in facts["entries"]):
            failures.append("text from the truncated attempt reached the conversation")
    finally:
        server.shutdown()

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
