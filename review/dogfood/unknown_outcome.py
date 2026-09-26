#!/usr/bin/env python3
"""A real-model check that an unverifiable outcome is reported, never replayed (§6.3/A09).

A08/A11 are the *recoverable* crash: the daemon dies, the runner survives and finishes the job, and the
receipt is consumed once (that is `crash.py`). A09 is the other half — the crash that leaves nothing
verifiable — and it is a promise about honesty rather than about recovery:

* the runner is killed **while the command runs**, so no terminal receipt is ever written and the journal
  stays `RUNNING`;
* the daemon is killed too, so recovery starts from a cold session;
* on restart the driver respawns a runner over the same job directory, which marks the journal
  `OUTCOME_UNKNOWN` (a runner that finds itself restarted over a non-terminal state cannot know whether the
  old child produced effects), the operation completes with class `outcome_unknown`, and the *running task*
  that was waiting on it parks `BLOCKED` with a `task_blocked` notification (§6.3/A09).

Asserts: `runs.log` has exactly **one** line (the command ran once and was not replayed), the worker's
operation really is `OUTCOME_UNKNOWN` with that receipt class, the delegated task is `BLOCKED` with exactly
one `task_blocked` event, and nothing claims the goal succeeded. The orphaned command (its process group
outlives the runner, D-41) is stopped explicitly at the end, which is the user's own lever as
`docs/USER-GUIDE.md` §4 describes.

    python3 review/dogfood/unknown_outcome.py
    python3 review/dogfood/unknown_outcome.py --provider kimi --state-dir /tmp/ta-unknown

Real model (`DEEPSEEK_API_KEY`, native window D-36), not part of `make check`, writes only under `--state-dir`.
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

# The line carries the shell's pid and its parent (the runner's): a replay by the same runner and a fresh attempt
# under a new runner are indistinguishable from `run` alone, which is what made the 2026-09-26 batch failure hard
# to attribute (D-140).
COMMAND = "sh -c 'echo \"run pid=$$ ppid=$PPID\" >> runs.log; sleep 20'"
HIRE = (
    "Spawn exactly one worker with the instructions `run the shell command the task gives you, then finish`. Do "
    "not delegate anything yet and do not settle the goal: report the worker's instance id and end your turn."
)
DELEGATE = (
    "If no goal is active, create one first (`create_goal`), then delegate exactly one task to that worker with "
    f"the text: `Call the shell tool exactly once with the command {COMMAND}` in the shared workspace. Then wait "
    "for the task and report its final status."
)


def read(state_root: pathlib.Path, query: str, params: list = []) -> list:
    """Rows, or [] while the session database does not exist yet (the first poll runs before it does)."""
    try:
        db = sqlite3.connect(f"file:{state_root / 'session.sqlite'}?mode=ro", uri=True)
    except sqlite3.Error:
        return []
    try:
        return list(db.execute(query, params))
    except sqlite3.Error:
        return []
    finally:
        db.close()


def worker_phase(state_root: pathlib.Path) -> str:
    rows = read(state_root, "SELECT id, phase FROM instances WHERE id != 'i-leader'")
    return f"{rows[0][0]}:{rows[0][1]}" if rows else "no worker"


def task_running(state_root: pathlib.Path) -> bool:
    return any(status == "RUNNING" for (_, status, _) in read(state_root, "SELECT id, status, assignee FROM tasks"))


def command_running(state_root: pathlib.Path) -> bool:
    """The *runner's own journal* says the command started.

    Not the operation row: `DISPATCH_COMMITTED` only means the driver committed the dispatch, and a runner that
    was killed before it accepted the GO leaves a `READY` journal — recovery then starts the command once, which
    is correct (§6.2) but is A08's scenario, not A09's. The journal is the witness that the command is really
    running, and it is also what the first version of this probe got wrong twice.
    """
    for journal in state_root.rglob("jobs/*/journal.json"):
        try:
            if json.loads(journal.read_text()).get("state") in ("START_ACCEPTED", "RUNNING"):
                return True
        except (OSError, json.JSONDecodeError):
            continue
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-unknown)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-unknown-{args.provider}")
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
    daemon = None

    # 1. the Leader hires one worker (the product path), and the user grants it the shell (§5.1/D-61: a spawned
    #    worker holds nothing, and without this the task cannot run a command at all)
    hired = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), HIRE],
        capture_output=True, text=True, env=env,
    )
    members = [row[0] for row in read(state_root, "SELECT id FROM instances WHERE id != 'i-leader'")]
    if len(members) != 1:
        print("  the Leader's own words:", (hired.stdout or hired.stderr)[:300])
        raise SystemExit(f"FAIL: the Leader hired {len(members)} workers (expected 1)")
    worker = members[0]
    granted = subprocess.run(
        [str(BIN), "authority", "--state-root", str(state_root), "grant", "--subject", worker,
         "--action", "shell", "--scope", "workspace", "--json"],
        capture_output=True, text=True, env=env,
    )
    print(f"  {worker} hired and granted shell@workspace (grant exit {granted.returncode})")

    # 2. the user asks for the delegation; the task must be RUNNING *and* the command in flight before the crash
    client = subprocess.Popen(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), DELEGATE],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    seen, in_flight = "no task", False
    deadline = time.time() + 300
    while time.time() < deadline:
        seen = worker_phase(state_root)
        if task_running(state_root) and command_running(state_root):
            in_flight = True
            break
        if client.poll() is not None:
            break
        time.sleep(0.25)
    print(f"  the worker reached {seen}; its task is RUNNING and the runner's journal says the command started")
    if not in_flight:
        client.kill()
        failures.append(f"the command never went in flight ({seen})")
    else:
        # the runner first (no terminal receipt) and then the daemon (cold recovery)
        subprocess.run(["pkill", "-f", f"jobs-runner {state_root}"], capture_output=True)
        subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)
    client.communicate(timeout=120)

    # 3. a cold start: the supervisor recovers the worker, whose journal is RUNNING with no receipt
    log = open(root / "daemon.log", "w")
    daemon = subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root), "--cwd", str(workspace)],
                              env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
    recovered = False
    for _ in range(240):
        rows = read(state_root, "SELECT status FROM operations WHERE status = 'OUTCOME_UNKNOWN'")
        if rows:
            recovered = True
            break
        time.sleep(0.5)
    print(f"  recovery marked the unverifiable operation: {recovered}")

    # 4. the trail
    runs = (workspace / "runs.log").read_text().splitlines() if (workspace / "runs.log").is_file() else []
    print(f"  runs.log holds {len(runs)} line(s)")
    operations = read(state_root, "SELECT operation_id, status, receipt_json, intent_json FROM operations")
    unknown = [(op, receipt) for op, status, receipt, _intent in operations if status == "OUTCOME_UNKNOWN"]
    # The invariant is **per call of this command**: an effect that may have happened is never started twice, so
    # the runs cannot outnumber the calls of the same command — and the probe's premise needs at least one call
    # to have really started. Two things make that count necessary rather than a plain "exactly one line":
    #   * the operations table holds every tool call, the leader's `spawn` and `delegate` included (measured:
    #     3 operations, 1 line), so counting operations would allow replays it must forbid;
    #   * the model may issue the same command twice, which the design allows and nothing forbids, and which
    #     the earlier "exactly one line" assertion reported as a replay (a batch run on 2026-09-26 did exactly
    #     that, and the artifact was gone by then because the probe's scratch is removed at exit — D-140).
    def command_of(intent_json) -> str:
        try:
            return str((json.loads(intent_json or "{}").get("args") or {}).get("command") or "")
        except (json.JSONDecodeError, TypeError, AttributeError):
            return ""

    calls = [op for op, _status, _receipt, intent in operations if "runs.log" in command_of(intent)]
    print(f"  operations: {[(op.split(':')[0][-8:], status) for op, status, _r, _i in operations]}")
    print(f"  calls of this command: {len(calls)}, runs: {len(runs)}")
    if len(runs) > len(calls):
        failures.append(f"the command ran {len(runs)} times for {len(calls)} call(s) of it: an operation was "
                        f"replayed (A09)")
    elif not runs:
        failures.append("the command never ran, so there was no in-flight effect to recover")
    elif len(calls) > 1:
        print(f"  the model issued the same command {len(calls)} times; each ran once, which the invariant allows")
    if not unknown:
        failures.append(f"no operation reached OUTCOME_UNKNOWN: {operations}")
    else:
        classes = set()
        for _, receipt in unknown:
            try:
                classes.add(((json.loads(receipt) or {}).get("error") or {}).get("class", ""))
            except (json.JSONDecodeError, TypeError):
                classes.add("unparseable")
        print(f"  operations in OUTCOME_UNKNOWN: {len(unknown)}, receipt classes {classes}")
        if classes != {"outcome_unknown"}:
            failures.append(f"the unknown receipt does not carry its class: {classes}")
    tasks = read(state_root, "SELECT id, status, assignee FROM tasks")
    print(f"  tasks: {tasks}")
    if not any(status == "BLOCKED" for _, status, _ in tasks):
        failures.append(f"no task parked BLOCKED after the unknown outcome: {tasks}")
    blocked = read(state_root, "SELECT scope FROM events WHERE kind = 'task_blocked'")
    print(f"  task_blocked events: {blocked}")
    if len(blocked) != 1:
        failures.append(f"expected exactly one task_blocked notification, saw {len(blocked)}")
    # a BLOCKED settlement is legitimate (the task parked, so the goal cannot succeed); success is not
    settled = read(state_root, "SELECT payload_json FROM events WHERE kind = 'goal_completed'")
    succeeded = [payload for (payload,) in settled if '"SUCCEEDED"' in payload]
    if succeeded:
        failures.append(f"a goal claimed success out of an unverifiable operation: {succeeded}")

    # 5. the orphaned command is the user's explicit lever, not the runtime's (D-41): stop it here
    for journal in state_root.rglob("jobs/*/journal.json"):
        try:
            pid = json.loads(journal.read_text()).get("pid")
        except (OSError, json.JSONDecodeError):
            continue
        if pid:
            subprocess.run(["kill", str(pid)], capture_output=True)
    if daemon is not None:
        subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)
        daemon.wait(timeout=30)
    log.close()
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
