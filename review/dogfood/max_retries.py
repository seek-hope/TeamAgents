#!/usr/bin/env python3
"""`[models.*].max_retries` is applied, per instance (D-240 measured it ignored; D-247 wired it).

The profile carries a retry budget and the driver retries a transient failure that many times before the turn
parks with `transient retries exhausted` (A19). D-240 measured the key *ignored*: one session constant served
every member, so a config asking for none still sent three requests, and this probe pinned that. This is the same
probe after the flip — the assertions are now that the *value the config asks for* decides the count:

* `max_retries = 0` → exactly one request (the sharpest form: "do not retry");
* `max_retries = 3` → four requests, so it is the value that is honoured, not a fixed number;
* the key omitted → the struct's own default (`core/src/models.rs::default_retries`, read from the code here),
  which is the budget the session passed for every instance before D-247, so nothing changes for a config that
  does not set it.

It needs no model and no credential: a local chat-completions server truncates **every** response before any
visible text, so every attempt is transient and the driver retries until its budget is gone; the probe counts
what the server saw and reads the session's own `attempts` rows.

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
MODELS = REPO / "core/src/models.rs"
KEY_VAR = "TEAMAGENTS_MAX_RETRIES_PROBE_KEY"
# the struct's default, read from its own function: a *simple* pattern, because a nested quantifier over the
# whole file (`(?:\*[^\n]*\n\s*)*`) backtracks catastrophically and hung this probe on its first run
DEFAULT_RETRIES = re.compile(r"fn default_retries\(\) -> i64 \{(.*?)\}", re.S)

HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# A tool-call id and nothing else: no visible text reached the user, so the attempt may be retried (§7,
# `providers_fake::truncated_stream_before_output_is_transient`) — and this server answers that way forever.
TRUNCATED = HEAD + b'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1"}]}}]}\n\n'


def default_budget() -> int:
    """The budget a config that omits the key gets, read from the struct's own default function."""
    found = DEFAULT_RETRIES.search(MODELS.read_text())
    # comments go first: the function carries the note that names the decision (`D-247`), and its digits are not
    # the budget — measured on this probe's own first green self-check, which read "the key omitted means 247"
    body = re.sub(r"//[^\n]*", "", found.group(1)) if found else ""
    number = re.search(r"\d+", body)
    return int(number.group(0)) if number else -1


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


def config(port: int, asked) -> str:
    budget = "" if asked is None else f"max_retries = {asked}\n"
    return f"""# max_retries probe (D-240/D-247): a local chat-completions server, no credentials, no network.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "max-retries-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
{budget}"""


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


def judgement(cases: list) -> list:
    """The probe's rule, as a function of what it measured — `--self-check` exercises it without a session.

    Each case is `(label, asked, expected_requests, seen, attempts)`, where `attempts` is the session's rows.
    """
    failures = []
    for label, asked, want, seen, rows in cases:
        if seen != want:
            what = "omitted" if asked is None else f"asked for {asked}"
            failures.append(f"{label} ({what}): expected {want} request(s), saw {seen}")
        if len(rows) != want or any(row != ("FAILED", "Transient") for row in rows):
            failures.append(f"{label}: expected {want} transient FAILED attempts, saw {rows}")
    return failures


def self_check() -> int:
    """Exercise `judgement` on the measured shapes and on the pre-D-247 shape (the constant for everyone)."""
    findings = []
    default = default_budget()
    if default < 0:
        findings.append(f"{MODELS.relative_to(REPO)}: `default_retries()` is not a literal any more, so the "
                        "budget a config that omits the key gets cannot be read — teach this probe where it moved")
    good = [("none", 0, 1, 1, [("FAILED", "Transient")]),
            ("three", 3, 4, 4, [("FAILED", "Transient")] * 4),
            ("default", None, default + 1, default + 1, [("FAILED", "Transient")] * (default + 1))]
    if judgement(good):
        findings.append("the shapes this build produces must pass")
    # the pre-D-247 shape: every case sent the session constant's count whatever the config said
    if not judgement([("none", 0, 1, 3, [("FAILED", "Transient")] * 3)]):
        findings.append("the ignored-key shape (three requests for a config asking none) must be reported")
    if not judgement([("default", None, default + 1, default + 2, [("FAILED", "Transient")] * (default + 2))]):
        findings.append("an off-by-one budget must be reported")
    for finding in findings:
        print(f"FAIL: {finding}")
    if not findings:
        print(f"self-check ok: the rule passes the measured shapes and reports the pre-D-247 one "
              f"(the key omitted means {default})")
    return 1 if findings else 0


def run_case(root: pathlib.Path, label: str, asked) -> tuple:
    """One session in its own state root; returns (label, asked, seen, attempt rows)."""
    state_root = root / label
    shutil.rmtree(state_root, ignore_errors=True)
    workspace = state_root / "ws"
    workspace.mkdir(parents=True)
    (state_root / "config/teamagents").mkdir(parents=True)
    (state_root / "config/teamagents/config.toml").write_text(config(PORT, asked))
    atexit.register(stop_daemon, state_root / "root")
    env = {"XDG_CONFIG_HOME": str(state_root / "config"), "XDG_STATE_HOME": str(state_root / "state"),
           "PATH": "/usr/bin:/bin", KEY_VAR: "not-a-key", "HOME": str(state_root)}
    AlwaysTruncated.seen = []
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root / "root"), "--full-auto", "--json", "--timeout", "90",
         "--cwd", str(workspace), "Do the work."],
        capture_output=True, text=True, env=env, timeout=240,
    )
    seen = len(AlwaysTruncated.seen)
    rows = attempts(state_root / "root") if (state_root / "root/session.sqlite").is_file() else []
    reason = str(json.loads(run.stdout).get("failure", "")) if run.stdout.strip().startswith("{") else run.stderr
    print(f"   {label}: exit={run.returncode} requests={seen} attempts={len(rows)} — {reason.strip()[:90]!r}")
    return label, asked, seen, rows


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

    global PORT
    server = ThreadingHTTPServer(("127.0.0.1", 0), AlwaysTruncated)
    PORT = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = pathlib.Path(args.state_dir or "/tmp/ta-max-retries")
    # The default scratch is not state anyone keeps: remove it at exit (D-138/D-131). An explicit --state-dir is
    # left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    failures: list[str] = []
    try:
        default = default_budget()
        print(f"the struct's default budget is {default} (read from {MODELS.relative_to(REPO)})")
        cases = []
        for label, asked, want in [("asked-none", 0, 1), ("asked-three", 3, 4), ("omitted", None, default + 1)]:
            label_, asked_, seen, rows = run_case(root, label, asked)
            cases.append((label_, asked_, want, seen, rows))
        failures += judgement(cases)
    finally:
        server.shutdown()
        AlwaysTruncated.seen = []

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


PORT = 0

if __name__ == "__main__":
    sys.exit(main())
