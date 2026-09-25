#!/usr/bin/env python3
"""A real-model check that a failing acceptance check really blocks a goal (A16).

One session, one configured `[[checks]]` entry that cannot pass (`test -f
review/dogfood/never-written`), and a task a model can actually complete (write a file). The runtime runs the
check at the completion boundary: the model's `finish` claims success, the check fails, the work goes back for
a bounded repair round, and the goal must end **BLOCKED with the failing check id** instead of reporting
success — which is the whole point of the completion gate (§8/A16).

The artifact decides the other half: the file the model was asked to write must really exist, so the run shows
"the work happened, the goal was not reported done" rather than "nothing happened".

    python3 review/dogfood/checks.py                  # fresh /tmp state root
    python3 review/dogfood/checks.py --state-dir /tmp/ta-checks --timeout 420

It is a real-model check: it needs `DEEPSEEK_API_KEY`, uses the model's native window (D-36), and writes only
under `--state-dir`.
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

CONFIG = """# Completion-gate dogfood (A16): the check below can never pass, so no run that
# reaches the completion boundary may report success.
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }

[[checks]]
id = "impossible"
command = "test -f never-written"
timeout = 60
"""

PROMPT = """Create the file hello.txt in this workspace whose content is exactly the line `hello from the model`.
Then report the task as finished."""


def session_facts(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    goals = list(db.execute("SELECT id, status, limits_json FROM goals ORDER BY rowid"))
    events = [(row[0], json.loads(row[1])) for row in db.execute("SELECT kind, payload_json FROM events ORDER BY sequence")]
    requests = list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]
    entries = [row[0] for row in db.execute("SELECT message_json FROM context_entries ORDER BY idx")]
    return {"goals": goals, "events": events, "requests": requests, "entries": entries}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-checks)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-checks")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), PROMPT],
        capture_output=True, text=True, env=env,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} goal={report.get('goal_status')}")
    if run.stderr.strip():
        print("stderr:", run.stderr.strip()[:300])

    facts = session_facts(state_root)
    print(f"model requests: {facts['requests']} | goals: {facts['goals']}")
    kinds = [kind for kind, _ in facts["events"]]
    for kind in ("check_round_registered", "completion_repair", "goal_completed"):
        print(f"  {kind}: {kinds.count(kind)}")

    # 1. the work really happened (otherwise this proves nothing about the gate)
    hello = workspace / "hello.txt"
    if hello.is_file() and hello.read_text().strip() == "hello from the model":
        print("hello.txt present with the expected content: the model did the work")
    else:
        failures.append(f"hello.txt is missing or wrong: {hello.read_text() if hello.is_file() else '(absent)'!r}")

    # 2. no success was reported: the check gated the goal
    if report.get("end") != "failed":
        failures.append(f"a run whose required check cannot pass must end failed, not {report.get('end')!r}")
    if run.returncode == 0:
        failures.append("the headless run exited 0 although its configured check never passed")
    statuses = {goal[1] for goal in facts["goals"]}
    if "SUCCEEDED" in statuses:
        failures.append(f"a goal was reported SUCCEEDED although its check cannot pass: {facts['goals']}")
    if "BLOCKED" not in statuses:
        failures.append(f"the goal did not end BLOCKED: {facts['goals']}")

    # 3. the settlement is BLOCKED, and the failing check is named in the ledger
    settled = [payload for kind, payload in facts["events"] if kind == "goal_completed"]
    if not settled or settled[-1].get("status") != "BLOCKED":
        failures.append(f"the goal did not settle BLOCKED: {settled}")
    else:
        print(f"the goal settled BLOCKED: {settled[-1].get('status')}")
    repairs = [payload for kind, payload in facts["events"] if kind == "completion_repair"]
    named = [failure for payload in repairs for failure in payload.get("failures", [])]
    if not named or any(failure.get("check_id") != "impossible" for failure in named):
        failures.append(f"the repair ledger does not name the failing check: {named}")
    else:
        print(f"the repair ledger names the failing check: {named[0]}")
    # and the model saw it: the check's output is a tool result (§8), not a silent gate
    seen = [entry for entry in facts["entries"] if "impossible" in entry]
    if not seen:
        failures.append("the failing check never appeared in the conversation")
    else:
        print(f"the conversation carries the check result: {seen[-1][:160]}")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
