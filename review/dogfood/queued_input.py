#!/usr/bin/env python3
"""A real-model check that a queued input is answered by its *own* turn (D-72).

Two runs share one session root. The first asks the model for work that takes a
turn the second run can arrive inside (a shell command that sleeps, then `finish`
with status success), so the second input is queued at the boundary instead of
being stored behind that turn's reply (D-63). The second run must report *its own*
answer — and must not report the goal the first run settled on its way out, which
is what a client that reads "the session settled" as "my prompt settled" does.

    python3 review/dogfood/queued_input.py                    # DeepSeek
    python3 review/dogfood/queued_input.py --provider kimi    # over `responses`

It is a real-model check: it needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi),
uses each model's native window (D-36), writes only under `--state-dir`, and is not
part of `make check`.
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
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
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

# The first turn settles the goal in its *first* response (no tool round in
# between), so the queued input can only land after that settlement — which is the
# attribution this probe is about. The model's own latency is the window the second
# run arrives in; the probe checks that it really arrived inside the turn.
SETTLING = """Reply with the single word DONE. Then report the task as finished by calling finish with
status success and the summary `the settling turn is done`."""
QUEUED = """Reply with exactly the word BANANA and nothing else. Do not call any tool."""


def exec_run(state_root: pathlib.Path, workspace: pathlib.Path, env: dict, prompt: str, timeout: int):
    return subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(timeout), "--cwd", str(workspace), prompt],
        capture_output=True, text=True, env=env,
    )


def wait_for_a_turn_in_flight(state_root: pathlib.Path, timeout_s: float = 60.0) -> str:
    """Poll the session until the leader is busy; returns the phase it was seen in."""
    db_path = state_root / "session.sqlite"
    deadline = time.time() + timeout_s
    last = "no database yet"
    while time.time() < deadline:
        try:
            db = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
            row = db.execute("SELECT phase FROM instances WHERE id = 'i-leader'").fetchone()
            db.close()
            if row:
                last = row[0]
                if last != "READY":
                    return last
        except sqlite3.Error as error:
            last = f"sqlite: {error}"
        time.sleep(0.25)
    return f"never busy ({last})"



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    leak_guard.stop_daemons(state_root)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-queued)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-queued-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text("skills_paths = []\n\n" + MODELS[args.provider])
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    first = subprocess.Popen(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), SETTLING],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    # arrive *inside* the first turn (the premise of this probe, so it is checked
    # rather than slept through)
    phase = wait_for_a_turn_in_flight(state_root)
    print(f"  run 1 was in flight in phase {phase}")
    second = exec_run(state_root, workspace, env, QUEUED, args.timeout)
    first_out, first_err = first.communicate(timeout=args.timeout + 60)
    elapsed = round(time.time() - started, 1)
    first_report = json.loads(first_out) if first_out.strip().startswith("{") else {}
    second_report = json.loads(second.stdout) if second.stdout.strip().startswith("{") else {}
    print(f"provider={args.provider} elapsed={elapsed}s")
    print(f"  run 1 (slow):   exit={first.returncode} end={first_report.get('end')} "
          f"goal={first_report.get('goal_status')}")
    print(f"  run 2 (queued): exit={second.returncode} end={second_report.get('end')} "
          f"goal={second_report.get('goal_status')} reply={second_report.get('reply')!r} "
          f"input_queued={second_report.get('input_queued')}")
    if first_err.strip().startswith("exec:"):
        print("  run 1 stderr:", first_err.strip()[:200])
    if second.stderr.strip().startswith("exec:"):
        print("  run 2 stderr:", second.stderr.strip()[:200])

    # 1. the first run's own outcome: the goal it settled
    if not phase.startswith(("MODEL_PENDING", "TOOLS_PENDING", "COMPLETION_PENDING")):
        failures.append(f"run 1 was never inside a turn, so nothing was queued behind it ({phase})")
    if first_report.get("goal_status") != "SUCCEEDED":
        failures.append(f"run 1 did not settle its own goal: {first_report.get('end')} "
                        f"{first_report.get('goal_status')!r} {first_report.get('failure')!r}")
    # 2. the second input really was queued behind that turn (otherwise this run
    #    proves nothing about attribution — the prompt would have been its own turn)
    if second_report.get("input_queued") is not True:
        failures.append(f"run 2 was not queued behind the running turn: {second_report}")
    # 3. the second run's outcome is its own answer, not the first run's settlement
    if second_report.get("end") != "reply" or "BANANA" not in (second_report.get("reply") or ""):
        failures.append(f"run 2 did not report its own reply: {second_report.get('end')} "
                        f"{second_report.get('reply')!r} {second_report.get('failure')!r}")
    if second_report.get("goal_status") is not None:
        failures.append(f"run 2 attributed the earlier turn's settlement to itself: "
                        f"{second_report.get('goal_status')!r}")
    if second.returncode != 0:
        failures.append(f"run 2 exited {second.returncode} for a plain reply of its own")

    db = sqlite3.connect(state_root / "session.sqlite")
    goal, = db.execute("SELECT status FROM goals")
    print(f"  the session's goal: {goal[0]}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
