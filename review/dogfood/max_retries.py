#!/usr/bin/env python3
"""`[models.*].max_retries` today: accepted, validated, and **not applied** (D-240).

The profile carries a retry count, and the driver's budget is the *session's* own constant: `engine/src/cli.rs`
builds the supervisor config with `max_retries: 2`, so a config that asks for a different number gets 2 anyway.
`review/config_keys.py` says so statically (its `masked:` bucket); this probe measures it over the real wire
stack, credential- and network-free, the way `instructions.py` pins D-102's promise:

* a local chat-completions server truncates **every** response before any visible text, so every attempt is
  transient and the driver retries until its budget is gone;
* the config asks for `max_retries = 0`, i.e. exactly one attempt;
* the probe then counts what the server saw. If the key were applied the run would send **one** request; today
  it sends the session constant's `max_retries + 1` (the first attempt plus its retries).

The constant is read from `engine/src/cli.rs`, not remembered, so the probe follows it when it changes and its
falsifier stays sharp: a run that sends `ASKED + 1` requests means the key is honoured now, and the check reports
that instead of passing — the D-240 record cannot go stale silently.

    python3 review/dogfood/max_retries.py
    python3 review/dogfood/max_retries.py --self-check      # the rule only, no session
    python3 review/dogfood/max_retries.py --state-dir /tmp/ta-max-retries
"""
import argparse
import atexit
import json
import pathlib
import re
import shutil
import sqlite3
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"
CLI = REPO / "engine/src/cli.rs"
KEY_VAR = "TEAMAGENTS_MAX_RETRIES_PROBE_KEY"
# What the config asks for: none, the sharpest form of "this setting is ignored".
ASKED = 0
SESSION_CONSTANT = re.compile(r"max_retries:\s*(\d+)")

HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# A tool-call id and nothing else: no visible text reached the user, so the attempt may be retried (§7,
# `providers_fake::truncated_stream_before_output_is_transient`) — and this server answers that way forever.
TRUNCATED = HEAD + b'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1"}]}}]}\n\n'


def applied_budget() -> int:
    """The budget the session builds for its drivers, read from the code — the fact the key is measured against."""
    found = set(SESSION_CONSTANT.findall(CLI.read_text()))
    return int(found.pop()) if len(found) == 1 else -1


class AlwaysTruncated(BaseHTTPRequestHandler):
    """Every POST gets the same transient truncation; the request count is the measurement."""

    protocol_version = "HTTP/1.0"
    seen: list = []

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            self.seen.append(json.loads(body.decode("utf-8", "replace")))
        except json.JSONDecodeError:
            self.seen.append({"unparseable": True})
        self.wfile.write(TRUNCATED)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int) -> str:
    return f"""# max_retries probe (D-240): a local chat-completions server, no credentials, no network.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "max-retries-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = {ASKED}
"""


def attempts(state_root: pathlib.Path) -> list:
    """`(status, error_class)` per attempt — the session's own record of what the driver did."""
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    try:
        return list(db.execute("SELECT status, error_class FROM attempts ORDER BY rowid"))
    finally:
        db.close()


def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started (registered with `atexit`, like every other probe here)."""
    leak_guard.stop_daemons(state_root)


def judgement(asked: int, applied: int, exit_code: int, text: str, requests_seen: int, attempt_rows: list) -> list:
    """The probe's rule, as a function of what it measured.

    Separate from the run because the branch that matters most cannot happen on this build: `requests_seen ==
    asked + 1` is what a *wired* key would produce, and a probe whose only live exercise is its passing branch is
    D-121's silent skip in probe form. `self_check` feeds this function that shape (and the other wrong ones) on
    every run, so the rule is exercised even while the product cannot produce it.
    """
    failures = []
    if asked == applied:
        failures.append(f"the probe cannot tell the key's number from the session's: both are {asked}")
    wanted = applied + 1
    if exit_code == 0 or "transient retries exhausted" not in text:
        failures.append(f"the run should end on an exhausted retry budget: exit={exit_code} {text[:200]!r}")
    if requests_seen == asked + 1:
        failures.append(f"the config's max_retries={asked} IS applied now ({requests_seen} request(s)): delete the "
                        "D-240 record, the `masked:` bucket and this probe's expectation together")
    elif requests_seen != wanted:
        failures.append(f"expected the session constant's {wanted} request(s), saw {requests_seen}")
    if len(attempt_rows) != wanted or any(row != ("FAILED", "Transient") for row in attempt_rows):
        failures.append(f"expected {wanted} transient FAILED attempts, saw {attempt_rows}")
    return failures


def self_check() -> int:
    """Exercise `judgement` on the measured shape and on the shapes a wrong build or a stale probe would give."""
    findings = []
    applied = applied_budget()
    if applied < 0:
        shown = CLI.relative_to(REPO) if CLI.is_relative_to(REPO) else CLI
        findings.append(f"{shown}: the session's `max_retries:` is not a single literal any more, "
                        "so the budget the key is measured against cannot be read — teach this probe where it moved")
        applied = 0
    exhausted = "transient retries exhausted: boom"
    measured = [("FAILED", "Transient")] * (applied + 1)
    if judgement(ASKED, applied, 1, exhausted, applied + 1, measured):
        findings.append("the shape this build produces (the session constant's requests) must pass")
    if not any("IS applied now" in one for one in judgement(ASKED, applied, 1, exhausted, ASKED + 1, [("FAILED", "Transient")])):
        findings.append("a wired key (one request) must be reported as such, not passed")
    if not judgement(ASKED, applied, 1, exhausted, applied, measured):
        findings.append("a request count that is neither the key's nor the constant's must fail")
    if not judgement(ASKED, applied, 0, "ok", applied + 1, measured):
        findings.append("a run that settled must fail the probe")
    if not judgement(ASKED, applied, 1, exhausted, applied + 1, [("COMPLETE", None)] * (applied + 1)):
        findings.append("attempts that are not transient failures must fail the probe")
    for finding in findings:
        print(f"FAIL: {finding}")
    if not findings:
        print(f"self-check ok: the rule passes the measured shape and reports a wired key "
              f"({ASKED} asked, {applied} applied)")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-max-retries)")
    parser.add_argument("--self-check", action="store_true", help="check the rule only, with no session")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if self_check():
        return 1
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    applied = applied_budget()

    server = ThreadingHTTPServer(("127.0.0.1", 0), AlwaysTruncated)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = pathlib.Path(args.state_dir or "/tmp/ta-max-retries")
    # The default scratch is not state anyone keeps: remove it at exit (D-138/D-131). An explicit --state-dir is
    # left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace, config_dir, state_root = root / "ws", root / "config/teamagents", root / "root"
    atexit.register(stop_daemon, state_root)
    failures: list[str] = []
    try:
        shutil.rmtree(root, ignore_errors=True)
        workspace.mkdir(parents=True)
        config_dir.mkdir(parents=True)
        text = config(port)
        (config_dir / "config.toml").write_text(text)
        if f"max_retries = {ASKED}" not in text:
            failures.append(f"the probe's premise is gone: the config it writes does not ask for {ASKED}")
        env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
               "PATH": "/usr/bin:/bin", KEY_VAR: "not-a-key", "HOME": str(root)}
        run = subprocess.run(
            [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "60",
             "--cwd", str(workspace), "Do the work."],
            capture_output=True, text=True, env=env, timeout=180,
        )
        report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
        seen = list(AlwaysTruncated.seen)
        facts = attempts(state_root) if (state_root / "session.sqlite").is_file() else []
        print(f"config asks for max_retries={ASKED}, the session constant is {applied}; exit={run.returncode} "
              f"end={report.get('end')} goal={report.get('goal_status')} requests_seen={len(seen)} attempts={facts}")
        print(f"   the run ends with: {str(report.get('failure') or run.stderr.strip())[:160]!r}")
        failures += judgement(ASKED, applied, run.returncode, str(report) + run.stderr, len(seen), facts)
        if len(seen) == applied + 1:
            print(f"   measured: the key is ignored — the budget is the session's own constant ({applied}) (D-240)")
    finally:
        server.shutdown()
        AlwaysTruncated.seen = []

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
