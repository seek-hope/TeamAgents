#!/usr/bin/env python3
"""A real-model check that a failing acceptance check really blocks a goal (A16).

One session, one configured `[[checks]]` entry that cannot pass (`exit 1`: a builtin, so no workspace content
changes its status — the file test this probe used to carry could be satisfied by writing the file the check
names, and a model did exactly that, D-146), and a task a model can actually complete (write a file). The
runtime runs the check at the completion boundary: the model's `finish` claims success, the check fails, the
work goes back for a bounded repair round, and the goal must end **BLOCKED with the failing check id** instead
of reporting success — which is the whole point of the completion gate (§8/A16).

The artifact decides the other half: the file the model was asked to write must really exist, so the run shows
"the work happened, the goal was not reported done" rather than "nothing happened".

    python3 review/dogfood/checks.py                  # fresh /tmp state root, DeepSeek
    python3 review/dogfood/checks.py --provider kimi  # the same scenario over `responses`
    python3 review/dogfood/checks.py --state-dir /tmp/ta-checks --timeout 420

It is a real-model check: it needs `DEEPSEEK_API_KEY` (or `KIMI_API_KEY` for `--provider kimi`), uses each
model's native window (D-36), and writes only under `--state-dir`.
"""
import argparse
import atexit
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

# The completion gate is protocol-agnostic in the design, so the harness can run the
# same scenario on a second provider: the wire for the *synthetic check entry* differs
# between protocols, and D-70 was exactly such a difference (DEEPSEEK).
MODELS = {
    "deepseek": """[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }
""",
    "kimi": """[models.leader_main]
provider = "kimi"
protocol = "responses"
model = "k3-256k"
base_url = "https://api.kimi.com/coding/v1"
api_key_env = "KIMI_API_KEY"
timeout = 300
max_retries = 1
generation_options = { reasoning_effort = "low" }
context_window = 262144
""",
}

# `exit 1` is a bash builtin: its exit status is the same whatever the workspace holds, so no action the model
# can take makes this gate pass. A workspace-relative file test is *not* such a command — the model wrote the
# file it named, on purpose, and the goal settled SUCCEEDED (D-146).
CHECK = """[[checks]]
id = "impossible"
command = "exit 1"
timeout = 60
"""


def config_text(provider: str) -> str:
    return (
        # Deliberately says nothing about whether the check can pass: this file is readable by the model, and
        # a comment that explains the gate answers the question the probe is asking (a real model quoted the
        # previous wording back while concluding it could not report success, D-146).
        "# Completion-gate dogfood (A16): the `[[checks]]` entry below is the runtime's completion gate.\n"
        "skills_paths = []\n\n" + MODELS[provider] + "\n" + CHECK
    )

PROMPT = """Create the file hello.txt in this workspace whose content is exactly the line `hello from the model`.
Then report the task as finished."""


def session_facts(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    goals = list(db.execute("SELECT id, status, limits_json FROM goals ORDER BY rowid"))
    events = [(row[0], json.loads(row[1])) for row in db.execute("SELECT kind, payload_json FROM events ORDER BY sequence")]
    requests = list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]
    entries = [row[0] for row in db.execute("SELECT message_json FROM context_entries ORDER BY idx")]
    return {"goals": goals, "events": events, "requests": requests, "entries": entries}



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-checks)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS),
                        help="which catalog entry the session runs on (the check round's wire differs per protocol)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-checks-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(config_text(args.provider))
    state_root = root / "root"
    atexit.register(stop_daemon, root)
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
    print(f"provider={args.provider} exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} "
          f"goal={report.get('goal_status')}")
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
        # Who decided the block is part of the evidence, not a pass/fail: the runtime blocks a goal whose
        # checks exhaust their repair rounds (`blocked_by: runtime`), and a model that reads a gate no
        # workspace state can satisfy may concede on its own first — the gate refused its success claim in
        # round 1 either way (D-146).
        rounds = kinds.count("check_round_registered")
        decider = settled[-1].get("blocked_by") or "the model's own blocked finish"
        print(f"the goal settled BLOCKED: {settled[-1].get('status')} after {rounds} check round(s), "
              f"settled by {decider}")
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
