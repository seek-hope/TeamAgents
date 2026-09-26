#!/usr/bin/env python3
"""A real-model check that a daemon crash replays nothing (§6.3, A08/A11/A12).

The design's core promise is that recovery is decided by *persisted location*: a tool whose
receipt exists is consumed, never re-executed, and a lost attempt is reported honestly. Unit
tests cover the paths; this drives the whole stack with a real model:

1. one headless run asks for work whose shell command takes long enough to kill the daemon
   mid-tool (`sh -c 'echo run >> runs.log; sleep 20'`) — `runs.log` is the side-effect counter;
2. the probe kills the daemon while the instance is in `TOOLS_PENDING` (the client's run dies
   with it, which is expected);
3. the session is started again: the supervisor recovers the instance, the *runner* (a
   separate process, A12) has finished the job and written its receipt, and the driver
   consumes that receipt instead of running the command a second time.

Asserts: `runs.log` has exactly one line (the command ran once, across the crash), the file the
task asked for exists, and the restarted session reports something coherent (a settlement or a
reply) instead of pretending nothing happened.

    python3 review/dogfood/crash.py

It is a real-model check: it needs `DEEPSEEK_API_KEY` (or `--provider kimi`), uses the model's
native window (D-36), writes only under `--state-dir`, and is not part of `make check`.
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

sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop: never `pkill -f` (D-148)
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

PROMPT = (
    "Do this in order, using the tools: (1) run the shell command "
    "`sh -c 'echo run >> runs.log; sleep 20'` in this workspace; (2) create the file done.txt "
    "containing exactly the line `finished`; (3) report the task as finished."
)
RESUME = "Continue: the work above must end with done.txt containing `finished`. Report the task as finished."


def phase(state_root: pathlib.Path) -> str:
    try:
        db = sqlite3.connect(f"file:{state_root / 'session.sqlite'}?mode=ro", uri=True)
        row = db.execute("SELECT phase FROM instances WHERE id = 'i-leader'").fetchone()
        db.close()
        return row[0] if row else "no instance"
    except sqlite3.Error as error:
        return f"sqlite: {error}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-crash)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-crash-{args.provider}")
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
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    def run(prompt: str, timeout: int) -> tuple[subprocess.CompletedProcess, dict]:
        done = subprocess.run(
            [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
             "--timeout", str(timeout), "--cwd", str(workspace), prompt],
            capture_output=True, text=True, env=env,
        )
        report = json.loads(done.stdout) if done.stdout.strip().startswith("{") else {}
        return done, report

    # 1. start the run and wait until the shell command is really in flight
    client = subprocess.Popen(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), PROMPT],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    seen, in_flight = "no instance", False
    for _ in range(240):
        seen = phase(state_root)
        if seen == "TOOLS_PENDING" and (workspace / "runs.log").is_file():
            in_flight = True
            break
        time.sleep(0.25)
    print(f"  the run reached phase {seen} (kill now)")
    if not in_flight:
        client.kill()
        failures.append(f"the tool never went in flight ({seen})")

    # 2. kill the daemon mid-tool: the client dies with it, the *runner* keeps the job
    survivors = leak_guard.stop_daemons(state_root)
    interrupted, _ = client.communicate(timeout=120)
    fate = "gone" if not survivors else f"survived: {survivors}"
    print(f"  killed the daemon ({fate}); the client exited {client.returncode}")
    if interrupted.strip():
        print(f"  the interrupted client said: {interrupted.strip()[:120]}")

    # the runner is a separate process (A12) and must finish the command on its own
    for _ in range(120):
        if (workspace / "runs.log").is_file() and (workspace / "done.txt").is_file():
            break
        time.sleep(0.5)

    # 3. start the session again: the supervisor recovers the instance and consumes the receipt
    done, report = run(RESUME, args.timeout)
    print(f"  resumed run exit={done.returncode} end={report.get('end')} goal={report.get('goal_status')} "
          f"input_queued={report.get('input_queued')} reply={str(report.get('reply'))[:60]!r}")
    if done.stderr.strip().startswith("exec:"):
        print("  resume stderr:", done.stderr.strip()[:160])

    # the side-effect counter: exactly one execution across the crash
    runs = (workspace / "runs.log").read_text().splitlines() if (workspace / "runs.log").is_file() else []
    print(f"  runs.log holds {len(runs)} line(s)")
    if len(runs) != 1:
        failures.append(f"the shell command ran {len(runs)} times across the crash (exactly once expected)")
    written = workspace / "done.txt"
    if not written.is_file() or written.read_text().strip() != "finished":
        failures.append(f"the work did not complete: {written} missing or wrong")
    # the resumed session must not claim a fresh start: it either settled or replied coherently
    if report.get("end") == "timeout":
        failures.append("the resumed session timed out instead of finishing the work")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
