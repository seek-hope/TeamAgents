#!/usr/bin/env python3
"""A graceful stop with a command in flight: DESIGN §9's normal shutdown, hermetically (D-152).

DESIGN §9 says what a normal daemon shutdown does — "freezes new dispatch, persists pending work and then
stops itself" — and until now nothing exercised it. `crash.py` measures the *crash* (SIGKILL: the runner keeps
the job, the session recovers the unverifiable operation as OUTCOME_UNKNOWN), and D-150 made SIGTERM reach the
shutdown path at all, asserting only that the daemon exits 0 and removes its socket. What a *graceful* stop
does to work that is genuinely in flight — a command already dispatched — was measured for the first time
while writing this probe, and this is that measurement as a re-runnable check:

* the daemon stops in bounded time, the client's stream ends, and the **operation is not settled** by the stop:
  it keeps the state it had (`DISPATCH_COMMITTED`), because the command is still running;
* the **runner** survives (A12) and finishes the command on its own, writing its journal;
* the next daemon over that state root **settles the operation from the runner's journal** — A11's "reconnect
  when verifiable", never a guess and never a replay;
* the command ran **once** (A08/A09: the effect is not re-executed), and the session is usable again: the goal
  settles with the user's continuation.

It needs **no credentials and no network**: a local chat-completions server answers the first turn with a
scripted `shell` tool call and every later turn with a `finish`, so the whole path runs over the engine's own
HTTP/SSE stack on loopback.

    python3 review/dogfood/shutdown.py
    python3 review/dogfood/shutdown.py --state-dir /tmp/ta-shutdown
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import signal
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_SHUTDOWN_PROBE_KEY"
HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# The command leaves a trace before it sleeps, so the probe can prove it ran and did not run twice.
COMMAND = "sh -c 'echo run >> runs.log; sleep 20'"
PROMPT = "Run the command the tools give you, then report the task as finished."
RESUME = "Continue: report the task as finished."


def stream(*deltas: dict) -> bytes:
    """One scripted chat-completions SSE response from delta objects."""
    body = b"".join(f"data: {json.dumps(delta)}\n\n".encode() for delta in deltas) + b"data: [DONE]\n\n"
    return HEAD + body


def tool_call(name: str, arguments: dict) -> bytes:
    return stream(
        {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": f"call-{name}", "type": "function",
                                                "function": {"name": name,
                                                             "arguments": json.dumps(arguments)}}]}}]},
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
         "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}},
    )


SHELL_TURN = tool_call("shell", {"command": COMMAND, "timeout": 60})
# Every later turn ends the goal: the recovery after the stop may itself continue the turn, so the script holds
# more finishes than the probe asks for and never depends on which turn consumes which.
FINISH_TURNS = [tool_call("finish", {"status": "success", "summary": "the command finished and was imported"})
                for _ in range(3)]


class FakeChat(BaseHTTPRequestHandler):
    """One scripted response per POST: the shell turn first, a finish for every turn after it."""

    protocol_version = "HTTP/1.0"
    seen: list = []

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            self.seen.append(json.loads(body.decode("utf-8", "replace")))
        except json.JSONDecodeError:
            self.seen.append({"unparseable": True})
        if len(self.seen) == 1:
            reply = SHELL_TURN
        elif len(self.seen) <= 1 + len(FINISH_TURNS):
            reply = FINISH_TURNS[len(self.seen) - 2]
        else:
            reply = b"HTTP/1.1 500 Script exhausted\r\ncontent-length: 0\r\n\r\n"
        self.wfile.write(reply)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int) -> str:
    return f"""# Shutdown probe (DESIGN §9): a local chat-completions server, no credentials, no network.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "shutdown-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = 1
"""


def facts(state_root: pathlib.Path) -> dict:
    """The session's own record: phases, operations, goals and the runner journals on disk."""
    out = {"phase": "none", "operations": [], "goals": [], "journal": None, "requests": 0}
    if not (state_root / "session.sqlite").exists():
        return out
    try:
        db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
        try:
            row = db.execute("SELECT phase FROM instances WHERE id = 'i-leader'").fetchone()
            out["phase"] = row[0] if row else "none"
            out["operations"] = list(db.execute("SELECT operation_id, status FROM operations ORDER BY rowid"))
            out["goals"] = list(db.execute("SELECT id, status FROM goals"))
            out["receipts"] = list(db.execute("SELECT status, receipt_json FROM operations ORDER BY rowid"))
            out["requests"] = db.execute("SELECT COUNT(*) FROM model_requests").fetchone()[0]
        finally:
            db.close()
    except sqlite3.Error as error:
        out["phase"] = f"sqlite: {error}"
    for journal in sorted((state_root / "instances/i-leader/jobs").glob("*/journal.json")):
        out["journal"] = json.loads(journal.read_text()).get("state")
    return out


def wait_for(predicate, seconds: float, what: str):
    """Poll until the predicate holds; the probe's own message says what it waited for."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.25)
    return predicate()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-shutdown)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    root = pathlib.Path(args.state_dir or "/tmp/ta-shutdown")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    shutil.rmtree(root, ignore_errors=True)
    workspace, state_root = root / "ws", root / "root"
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    (root / "config/teamagents/config.toml").write_text(config(port))
    atexit.register(leak_guard.stop_daemons, state_root)
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": os.environ.get("PATH", "/usr/bin:/bin"), KEY_VAR: "not-a-key", "HOME": str(root)}
    failures: list[str] = []

    # 1. a real turn dispatches the command and is in flight
    client = subprocess.Popen(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "120",
         "--cwd", str(workspace), PROMPT],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
    in_flight = wait_for(lambda: facts(state_root)["phase"] == "TOOLS_PENDING"
                         and (workspace / "runs.log").is_file() and leak_guard.runner_pids(state_root), 60, "in flight")
    observed = facts(state_root)
    print(f"1. in flight: phase={observed['phase']} operations={observed['operations']} "
          f"runners={len(leak_guard.runner_pids(state_root))}")
    if not in_flight:
        failures.append(f"the command never went in flight: {observed}")

    # 2. SIGTERM: the graceful stop. It must be bounded, must not settle the operation, and must leave the
    #    runner that owns the running command alive (A12).
    pids = [pid for pid, _args in leak_guard.daemon_pids(state_root)]
    if len(pids) != 1:
        failures.append(f"expected exactly one daemon to stop, found {pids}")
    started = time.time()
    if pids:
        os.kill(pids[0], signal.SIGTERM)
    stopped = wait_for(lambda: not leak_guard.daemon_pids(state_root), 30, "the daemon to stop")
    stopped_after = round(time.time() - started, 1)
    out, err = client.communicate(timeout=60)
    after_stop = facts(state_root)
    open_states = ("DISPATCH_COMMITTED", "PREPARED", "RUNNING")
    print(f"2. graceful stop: daemon gone after {stopped_after}s, client exit={client.returncode} "
          f"({err.strip()[:70]!r}), operations={after_stop['operations']}, "
          f"runners={len(leak_guard.runner_pids(state_root))}")
    if not stopped or stopped_after > 30:
        failures.append(f"the graceful stop was not bounded: gone={stopped} after {stopped_after}s")
    if not after_stop["operations"] or any(status not in open_states for _id, status in after_stop["operations"]):
        failures.append(f"the stop settled an operation that is still running: {after_stop['operations']}")
    if not leak_guard.runner_pids(state_root):
        failures.append("the runner died with the daemon: the command cannot finish on its own (A12)")
    if not (workspace / "runs.log").is_file():
        failures.append("the command never left its trace")

    # 3. the runner finishes the command while no daemon is running (§6.2)
    journal = wait_for(lambda: facts(state_root)["journal"] if facts(state_root)["journal"] in
                       ("SUCCEEDED", "FAILED") else None, 90, "the runner's journal")
    print(f"3. the runner finished it alone: journal={journal}")
    if journal != "SUCCEEDED":
        failures.append(f"the runner did not finish the command on its own: {journal}")

    # 4. the next daemon settles that operation from the journal — A11's "reconnect when verifiable"
    daemon = subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    settled = wait_for(lambda: [row for row in facts(state_root)["operations"] if row[1] not in open_states], 60,
                       "recovery to settle the operation")
    recovery = facts(state_root)
    settled_goal = wait_for(lambda: facts(state_root)["goals"] and facts(state_root)["goals"][0][1] == "SUCCEEDED",
                            60, "the goal")
    print(f"4. recovery settled it: operations={recovery['operations']}")
    if not settled or any(status != "SUCCEEDED" for _id, status in settled):
        failures.append(f"recovery did not settle the interrupted operation from the journal: {recovery['operations']}")
    for status, receipt in recovery.get("receipts", []):
        ok = json.loads(receipt or "{}").get("ok")
        if status == "SUCCEEDED" and ok is not True:
            failures.append(f"the settled operation has no runner receipt saying ok: {receipt}")

    # 5. the session is usable again, and the command was never replayed (A08/A09)
    resumed = subprocess.run([str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
                              "--timeout", "120", "--cwd", str(workspace), RESUME],
                             capture_output=True, text=True, env=env, timeout=180)
    report = json.loads(resumed.stdout) if resumed.stdout.strip().startswith("{") else {}
    final = facts(state_root)
    # the trace may be missing when the premise above already failed: that is a reported failure, not a
    # traceback (a probe that crashes hides the FAIL line the reader needs)
    runs = (workspace / "runs.log").read_text().split() if (workspace / "runs.log").is_file() else []
    route = "the recovery's own turn settled the goal" if settled_goal else "the user's continuation settled it"
    print(f"5. the user continues: exit={resumed.returncode} end={report.get('end')} "
          f"goal={report.get('goal_status')} goals={final['goals']} runs.log={runs} "
          f"requests seen by the server={len(FakeChat.seen)} — {route}")
    if not final["goals"] or final["goals"][0][1] != "SUCCEEDED":
        failures.append(f"the goal did not settle after the stop: exit={resumed.returncode} "
                        f"{report.get('failure')} {final['goals']}")
    # A run reports its *own* input's outcome (D-72): the input that arrives after the recovery already settled
    # the goal cannot claim that settlement, so `unsettled` is the honest word for it — and a timeout or a hang
    # is not. Either route is fine; what is not fine is a run that reports the earlier settlement as its own.
    if resumed.returncode not in (0, 1) or report.get("end") not in ("completed", "reply", "unsettled"):
        failures.append(f"the continuation ended outside the documented contract: exit={resumed.returncode} "
                        f"end={report.get('end')!r} failure={report.get('failure')}")
    if report.get("end") == "unsettled" and report.get("goal_status") == "SUCCEEDED":
        failures.append("a run reported the settlement of an earlier turn as its own outcome (D-72)")
    if report.get("end") == "unsettled" and not settled_goal:
        failures.append("the run ended unsettled although nothing had settled the goal before it")
    if runs != ["run"]:
        failures.append(f"the command did not run exactly once across the stop: {runs}")
    if any(status in open_states for _id, status in final["operations"]):
        failures.append(f"an operation is left open: {final['operations']}")
    if any(entry.get("tool_calls") and "shell" in json.dumps(entry) for entry in FakeChat.seen[1:]):
        failures.append("the model was asked for a second shell call: the turn was replayed")

    leak_guard.stop_daemons(state_root)
    left = (len(leak_guard.daemon_pids(state_root)), len(leak_guard.runner_pids(state_root)))
    print(f"6. cleaned up: daemons/runners left = {left}")
    if left != (0, 0):
        failures.append(f"the probe left processes behind: {left}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
