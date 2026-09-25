#!/usr/bin/env python3
"""The goal deadline, end to end: the daemon refuses past it and the client says so (A35).

A35's row has formal coverage (`V2Control`'s deadline gate plus the refuted control
`MC_control_deadline.cfg`) and a config test; what a user meets is the *product* path: a session whose
`[limits] deadline_minutes` has passed must refuse the next request — no model call — and the headless client
must report that refusal with the runtime's reason instead of waiting out its own deadline and answering
`124` (D-97).

    python3 review/dogfood/deadline.py                 # ~70 s: one minute of waiting is the point
    python3 review/dogfood/deadline.py --state-dir /tmp/ta-deadline

The probe runs one short turn (which creates the goal with its one-minute deadline), waits until the deadline
the runtime recorded has passed, and runs a second turn with a `--check` attached. It then asserts on the
session itself: exactly one model request (the second turn never reached a model), a `goal_deadline_refused`
event, the leader `PARKED` with "goal … deadline passed", the client's `failure` carrying the same words, and
no acceptance command run for the refused turn (D-96: an unfinished turn verifies nothing).

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36) and writes only under `--state-dir`.
"""
import argparse
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

CONFIG = """# Deadline dogfood (A35): a real model on the native context window (D-36).
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 120
max_retries = 2
generation_options = { reasoning_effort = "high" }

[limits]
deadline_minutes = 1
"""

FIRST = "Say the single word ok."
SECOND = "Say the single word again."


def run_exec(state_root: pathlib.Path, workspace: pathlib.Path, env: dict, prompt: str,
             checks: list[str], timeout: int = 120) -> tuple[subprocess.CompletedProcess, dict]:
    args = [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
            "--timeout", str(timeout), "--cwd", str(workspace)]
    for check in checks:
        args += ["--check", check]
    args.append(prompt)
    completed = subprocess.run(args, capture_output=True, text=True, env=env, timeout=timeout + 60)
    report = json.loads(completed.stdout) if completed.stdout.strip().startswith("{") else {}
    return completed, report


def session(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    goals = list(db.execute("SELECT id, status, deadline FROM goals ORDER BY rowid"))
    events = [(row[0], row[1]) for row in db.execute("SELECT kind, payload_json FROM events ORDER BY sequence")]
    requests = list(db.execute("SELECT request_id, status FROM model_requests"))
    instances = list(db.execute("SELECT id, lifecycle, phase FROM instances"))
    return {"goals": goals, "events": events, "requests": requests, "instances": instances}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-deadline)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-deadline")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []
    marker = workspace / "check-ran"

    # 1. a short turn creates the goal and its one-minute deadline
    started = time.time()
    first, report = run_exec(state_root, workspace, env, FIRST, [])
    print(f"1. first turn: exit={first.returncode} end={report.get('end')} ({round(time.time() - started, 1)}s)")
    if first.returncode != 0:
        failures.append(f"the first turn should succeed, got {first.returncode}: {report}")
    facts = session(state_root)
    goal = facts["goals"][0] if facts["goals"] else None
    if goal is None or goal[2] is None:
        failures.append(f"the goal carries no deadline: {facts['goals']}")
        return 1
    deadline = goal[2]
    print(f"   goal {goal[0]} is {goal[1]} with an absolute deadline {round(deadline - time.time())}s ahead")

    # 2. wait for the deadline the runtime recorded, then ask again
    wait = max(0.0, deadline - time.time()) + 2.0
    print(f"2. waiting {round(wait)}s for the deadline to pass…")
    time.sleep(wait)
    started = time.time()
    second, report = run_exec(state_root, workspace, env, SECOND, [f"touch {marker}"])
    elapsed = round(time.time() - started, 1)
    print(f"   second turn: exit={second.returncode} end={report.get('end')} "
          f"failure={report.get('failure')!r} ({elapsed}s)")
    if second.returncode != 1:
        failures.append(f"a request past the goal deadline must exit 1, got {second.returncode}")
    if report.get("end") != "failed":
        failures.append(f"the refusal is this run's outcome, not {report.get('end')!r}")
    failure = report.get("failure") or ""
    if "deadline passed" not in failure:
        failures.append(f"the failure does not name the deadline: {failure!r}")
    if elapsed > 30:
        failures.append(f"a refused request must end the run at once, took {elapsed}s")
    if marker.exists():
        failures.append("an acceptance command ran for a turn that never started")
    if report.get("verification") != [] or report.get("verification_path") is not None:
        failures.append(f"a refused run verified something: {report.get('verification')}")

    # 3. the session itself: no model call, the refusal recorded, the leader parked with the reason
    facts = session(state_root)
    if len(facts["requests"]) != 1:
        failures.append(f"the refused turn reached a model: {facts['requests']}")
    else:
        print("   the session holds exactly one model request (the refused turn never called a model)")
    refused = [payload for kind, payload in facts["events"] if kind == "goal_deadline_refused"]
    if not refused:
        failures.append("the session did not record the refusal")
    else:
        print(f"   the session recorded the refusal: {refused[-1]}")
    parked = [(row[0], row[1]) for row in facts["instances"] if row[1] == "PARKED"]
    reasons = [payload for kind, payload in facts["events"] if kind == "instance_lifecycle"]
    reason = ""
    if reasons:
        try:
            reason = json.loads(reasons[-1]).get("reason", "")
        except json.JSONDecodeError:
            reason = "unparseable"
    if not parked or "deadline passed" not in reason:
        failures.append(f"the leader is not parked with the deadline reason: {parked} {reason!r}")
    else:
        print(f"   the leader is {parked[0][1]} with reason {reason!r}")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
