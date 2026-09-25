#!/usr/bin/env python3
"""A real-model check that a settlement's own words ride the *next* turn (D-71).

When a goal settles, the runtime appends its closing note to the member's context
(`runtime: goal … closed as SUCCEEDED`). It is the runtime's own word, stored under the
`runtime` kind in the user's voice, and it stays in the conversation — so the next turn
in that session sends a transcript that carries it. Both wires must accept that.

The probe therefore runs two turns against one state root:

1. a turn the model ends with `finish(status = success)` — the goal settles `SUCCEEDED`
   and the runtime's note joins the context;
2. a plain turn in the same session, whose request carries that note.

Both turns must succeed, and the second must be an ordinary `reply` (exit 0) — a strict
endpoint rejecting the shape shows up here as a failed request, not as a hang.

    python3 review/dogfood/runtime_note.py                      # DeepSeek
    python3 review/dogfood/runtime_note.py --provider kimi      # over `responses`
    python3 review/dogfood/runtime_note.py --providers deepseek,kimi

It is a real-model check: it needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi), uses
each model's native window (D-36), writes only under `--state-dir`-derived roots, and is
not part of `make check`.
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

SETTLE = "Write the single word OK in your reply, then call finish with status success."
FOLLOW_UP = "Reply with exactly the word TWO and nothing else."


def exec_turn(state_root: pathlib.Path, workspace: pathlib.Path, env: dict, prompt: str, timeout: int):
    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(timeout), "--cwd", str(workspace), prompt],
        capture_output=True, text=True, env=env,
    )
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    return run, report, round(time.time() - started, 1)


def check(provider: str, root: pathlib.Path, timeout: int, failures: list[str]) -> None:
    config = root / "config/teamagents"
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    config.mkdir(parents=True)
    workspace.mkdir(parents=True)
    (config / "config.toml").write_text("skills_paths = []\n\n" + MODELS[provider])
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}

    run, report, elapsed = exec_turn(state_root, workspace, env, SETTLE, timeout)
    print(f"provider={provider} turn 1: exit={run.returncode} elapsed={elapsed}s "
          f"end={report.get('end')} goal={report.get('goal_status')}")
    if report.get("goal_status") != "SUCCEEDED":
        failures.append(f"{provider}: turn 1 did not settle SUCCEEDED "
                        f"({run.returncode}, {report.get('end')}, {report.get('failure')!r})")
        return

    run, report, elapsed = exec_turn(state_root, workspace, env, FOLLOW_UP, timeout)
    print(f"provider={provider} turn 2: exit={run.returncode} elapsed={elapsed}s "
          f"end={report.get('end')} reply={report.get('reply')!r}")
    if run.returncode != 0 or report.get("end") != "reply":
        failures.append(f"{provider}: the turn after a settlement failed "
                        f"({run.returncode}, {report.get('end')}, {report.get('failure')!r})")

    db = sqlite3.connect(state_root / "session.sqlite")
    notes = list(db.execute("SELECT id, kind, message_json FROM context_entries WHERE kind = 'runtime'"))
    if not notes:
        failures.append(f"{provider}: the settlement note is not an entry of kind `runtime`")
    else:
        print(f"provider={provider} the runtime's closing note in the context: {notes[0][0]} {notes[0][2][:80]}")
        if '"role":"user"' not in notes[0][2]:
            failures.append(f"{provider}: the runtime's note is not in the user's voice: {notes[0][2][:120]}")



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--providers", help="comma-separated list, overrides --provider")
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-runtime-note)")
    parser.add_argument("--timeout", type=int, default=300, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    providers = (args.providers.split(",") if args.providers else [args.provider])
    base = pathlib.Path(args.state_dir or "/tmp/ta-runtime-note")
    failures: list[str] = []
    for provider in providers:
        provider = provider.strip()
        key_env = "KIMI_API_KEY" if provider == "kimi" else "DEEPSEEK_API_KEY"
        if not os.environ.get(key_env, "").strip():
            print(f"provider={provider} skipped: {key_env} is not set in this environment")
            continue
        check(provider, base / provider, args.timeout, failures)
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
