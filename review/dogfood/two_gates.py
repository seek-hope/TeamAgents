#!/usr/bin/env python3
"""The runtime's gate and the client's gate in one run (D-49/D-50, A16/A17 — which decides what).

The product has two acceptance mechanisms and they answer different questions:

* `[[checks]]` in the user config gate the **goal**: the driver runs them at the completion boundary, a failure
  enters a bounded repair round and then blocks the goal (§8, A16);
* `exec --check` is the **client's** own command, run after the turn, and it decides the **exit code** (D-49).

Nothing documented what happens when both are configured at once, and nothing had driven that combination
live. This probe does, with two scenarios that separate the two roles:

1. a runtime check that cannot pass in any workspace state (`exit 1`, a builtin — a file test would be
   satisfiable by writing the file the check names, which is how this scenario first failed, D-146) + a client
   check that passes → the **goal** is `BLOCKED` and the exit code is `1`, while the client's verdict is
   `ok: true` — the goal's gate is not the client's, and the run still fails;
2. a runtime check that passes + a client check that fails → the goal really settles `SUCCEEDED` and the run
   still exits `1`: the client's command is the last word on a finished turn.

    python3 review/dogfood/two_gates.py

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36), runs each scenario on its own fresh state root
(runtime checks belong to the goal) and writes only under `--state-dir`.
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

MODEL = """[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }
"""

PROMPT = ("Create the file hello.txt in this workspace whose content is exactly the line hello. Then report "
          "the task as finished.")


def config_text(runtime_check: str) -> str:
    return f"""# Two-gate dogfood (A16/D-49): a real model on the native context window (D-36).
skills_paths = []

{MODEL}
[[checks]]
id = "runtime-gate"
command = "{runtime_check}"
timeout = 60
"""


def run_scenario(name: str, root: pathlib.Path, runtime_check: str, client_check: str,
                 env_base: dict) -> tuple[subprocess.CompletedProcess, dict, dict]:
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(config_text(runtime_check))
    state_root = root / "root"
    atexit.register(stop_daemon, state_root)
    env = {**env_base, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    started = time.time()
    run = subprocess.run([str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
                          "--timeout", "240", "--cwd", str(workspace), "--check", client_check, PROMPT],
                         capture_output=True, text=True, env=env, timeout=300)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    facts = {
        "goals": list(db.execute("SELECT id, status FROM goals")),
        # a goal can end BLOCKED either because the runtime exhausted its repair rounds (then the event
        # carries the reason) or because the model itself reported `blocked` after seeing the failing check —
        # so the probe reads the *check ledger* for the check's name and the completions for the status
        "completions": [row[1] for row in db.execute(
            "SELECT kind, payload_json FROM events WHERE kind IN ('goal_completed', 'goal_blocked') ORDER BY sequence")],
        "repairs": [row[0] for row in db.execute(
            "SELECT payload_json FROM events WHERE kind = 'completion_repair' ORDER BY sequence")],
        "entries": [row[0] for row in db.execute("SELECT message_json FROM context_entries ORDER BY idx")],
        "elapsed": round(time.time() - started, 1),
    }
    return run, report, facts



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-two-gates)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    base = pathlib.Path(args.state_dir or "/tmp/ta-two-gates")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, base, ignore_errors=True)
    shutil.rmtree(base, ignore_errors=True)
    env_base = dict(os.environ)
    failures: list[str] = []

    # 1. the runtime gate blocks the goal, the client's check still passes
    run, report, facts = run_scenario("runtime-fails", base / "a", "exit 1", "test -f hello.txt", env_base)
    print(f"1. runtime gate fails, client check passes: exit={run.returncode} end={report.get('end')} "
          f"goal={report.get('goal_status')} ({facts['elapsed']}s)")
    verdicts = report.get("verification") or []
    print(f"   client verdicts: {[(v.get('command'), v.get('ok')) for v in verdicts]}")
    if run.returncode != 1:
        failures.append(f"a blocked goal must exit 1, got {run.returncode}")
    if report.get("goal_status") != "BLOCKED":
        failures.append(f"the goal should be BLOCKED, got {report.get('goal_status')!r}")
    if len(verdicts) != 1 or verdicts[0].get("ok") is not True:
        failures.append(f"the client check should still run and pass: {verdicts}")
    named = [failure.get("check_id", "") for payload in facts["repairs"]
             for failure in json.loads(payload).get("failures", [])]
    if "runtime-gate" not in named:
        failures.append(f"the repair ledger does not name the runtime check: {named}")
    else:
        print(f"   the repair ledger names the runtime check ({len(named)} round(s) reported)")
    statuses = [json.loads(payload).get("status", "") for payload in facts["completions"]]
    if "BLOCKED" not in statuses:
        failures.append(f"the completion event does not say BLOCKED: {statuses}")
    if not (base / "a/root/verification.json").is_file():
        failures.append("the client's ledger was not written for the finished turn")
    if not any("runtime-gate" in entry for entry in facts["entries"]):
        failures.append("the model was never told about the runtime check's failure")

    # 2. the runtime gate passes, the client's check fails: settled goal, exit 1
    run, report, facts = run_scenario("client-fails", base / "b", "test -f hello.txt", "test -f missing.txt",
                                      env_base)
    print(f"2. runtime gate passes, client check fails: exit={run.returncode} end={report.get('end')} "
          f"goal={report.get('goal_status')} ({facts['elapsed']}s)")
    verdicts = report.get("verification") or []
    print(f"   client verdicts: {[(v.get('command'), v.get('ok'), v.get('exit_code')) for v in verdicts]}")
    if report.get("goal_status") != "SUCCEEDED":
        failures.append(f"the runtime gate passed, so the goal should settle SUCCEEDED: {report.get('goal_status')!r}")
    if run.returncode != 1:
        failures.append(f"a failing client check must fail the run even after a settlement, got {run.returncode}")
    if len(verdicts) != 1 or verdicts[0].get("ok") is not False:
        failures.append(f"the client check's failure is not recorded: {verdicts}")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
