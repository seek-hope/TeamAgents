#!/usr/bin/env python3
"""The goal budget, end to end, without spending anything (A18).

A18 has an in-process test for the ceiling (`control::a_worker_shares_the_budget_of_the_goal_its_queue_serves`)
and the product-path tests that the config reaches the goal (`cli::configured_limits_reach_the_goal_and_really_
bound_the_session`, `cli::a_tiny_configured_ceiling_parks_the_session_instead_of_running_it`). What neither
shows is the *live* shape a user meets: a session whose ceiling is below one request must refuse **before any
model call** — no money spent, no request row — park the leader with the runtime's own reason, and report that
refusal through the headless client instead of waiting out its deadline (D-97).

This probe is deliberately **credential-free and network-free**: the refusal happens before the provider is
asked for anything, so a dummy key in the environment is enough. It is the rare live probe that can be re-run
anywhere, including on a machine that has no API key at all.

    python3 review/dogfood/budget.py
    python3 review/dogfood/budget.py --state-dir /tmp/ta-budget --max-tokens 500

It writes only under `--state-dir` and stops the daemon it started.
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
KEY_VAR = "TEAMAGENTS_BUDGET_PROBE_KEY"


def config(max_tokens: int) -> str:
    return f"""# Budget dogfood (A18): a ceiling below one request, so the refusal is the whole run.
# The key is a dummy: the runtime refuses before it ever asks a provider (§7).
skills_paths = []

[models.leader_main]
provider = "openai"
model = "budget-probe"
api_key_env = "{KEY_VAR}"
base_url = "http://127.0.0.1:1/v1"

[limits]
max_total_tokens = {max_tokens}
"""


def session(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    goals = list(db.execute("SELECT id, status, known_usage_json, unknown_usage, limits_json FROM goals"))
    requests = list(db.execute("SELECT request_id, status FROM model_requests"))
    instances = list(db.execute("SELECT id, lifecycle, phase FROM instances"))
    events = [(row[0], row[1]) for row in db.execute("SELECT kind, payload_json FROM events ORDER BY sequence")]
    return {"goals": goals, "requests": requests, "instances": instances, "events": events}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-budget)")
    parser.add_argument("--max-tokens", type=int, default=1000, help="the ceiling the config sets")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    root = pathlib.Path(args.state_dir or "/tmp/ta-budget")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(config(args.max_tokens))
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           KEY_VAR: "not-a-key"}
    failures: list[str] = []
    marker = workspace / "check-ran"

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "60",
         "--cwd", str(workspace), "--check", f"touch {marker}", "Do some work."],
        capture_output=True, text=True, env=env, timeout=180,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"  the run: exit={run.returncode} end={report.get('end')} failure={report.get('failure')!r} ({elapsed}s)")
    if run.returncode != 1:
        failures.append(f"a goal whose ceiling is below one request must end the run with 1, got {run.returncode}")
    if report.get("end") != "failed":
        failures.append(f"the refusal is this run's outcome, not {report.get('end')!r}")
    failure = report.get("failure") or ""
    if "budget exceeded" not in failure or f"max {args.max_tokens}" not in failure:
        failures.append(f"the failure does not name the ceiling: {failure!r}")
    if elapsed > 30:
        failures.append(f"a refused request must end the run at once, took {elapsed}s")
    if marker.exists():
        failures.append("an acceptance command ran for a turn that never started")
    if report.get("verification") != [] or report.get("verification_path") is not None:
        failures.append(f"a refused run verified something: {report.get('verification')}")

    facts = session(state_root)
    if facts["requests"]:
        failures.append(f"the refused turn reached a model (money spent): {facts['requests']}")
    else:
        print("  no model request was registered: nothing was spent")
    refused = [payload for _, payload in facts["events"] if _ == "budget_refused"]
    if not refused:
        failures.append(f"the session did not record the refusal: {[kind for kind, _ in facts['events']]}")
    else:
        print(f"  the session recorded the refusal: {refused[-1]}")
    parked = [(row[0], row[1]) for row in facts["instances"] if row[1] == "PARKED"]
    reasons = [payload for kind, payload in facts["events"] if kind == "instance_lifecycle"]
    reason = ""
    if reasons:
        try:
            reason = json.loads(reasons[-1]).get("reason", "")
        except json.JSONDecodeError:
            reason = "unparseable"
    if not parked or "budget exceeded" not in reason:
        failures.append(f"the leader is not parked with the budget reason: {parked} {reason!r}")
    else:
        print(f"  the leader is {parked[0][1]} with reason {reason!r}")
    goals = facts["goals"]
    if not goals:
        failures.append("no goal was created for the refused turn")
    else:
        print(f"  the goal carries the ceiling: {goals[0][4]}")

    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
