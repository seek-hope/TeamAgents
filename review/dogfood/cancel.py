#!/usr/bin/env python3
"""A real-model check of the user's stop lever (A13): does a running command really stop?

A13's evidence is offline: `jobs_runner::cancel_running_stops_the_process_group` and
`v2_driver::user_cancel_stops_a_running_job` drive a scripted provider and a scripted runner client. The
product question a user actually asks is different, and it has two parts this probe answers with a real
model, a real shell command and the real runner process:

* **the effect.** A member runs `while true; do echo tick >> heartbeat.txt; sleep 1; done`. The file growing
  once a second is the *only* thing the probe trusts: it is produced by the process the runner spawned, so
  "the file stopped growing" means "the process group is gone", not "a row changed".
* **the receipt.** After the lever is pulled the operation's receipt must say the cancellation reached the
  runner (`class: cancelled`), not that the command ran into its own timeout (`class: timeout`) — those two
  are indistinguishable from the outside unless the probe reads the class, and only the first one is the
  user's lever working.

The flow keeps the model's part minimal and puts the rest on user surfaces: the Leader hires the worker
(one small turn — the only product path that creates a member with a real profile), the probe grants it
`shell@workspace` with `teamagents authority grant` (§5.1: a spawned worker holds none), submits the
instruction to the worker directly as the user (A21's direct input, the same command the TUI sends), and then
pulls `teamagents instances terminate --id … --yes` — the only stop lever in the CLI today. Cancelling a
*task* is a different, delegation-level action: it releases the delegator and does **not** stop the assignee's
running operation (§6.4's process-group stop is a request against an *operation*); the offline test
`v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance` pins that side.

    python3 review/dogfood/cancel.py
    python3 review/dogfood/cancel.py --state-dir /tmp/ta-cancel --stop-window 30

It is a real-model check (`DEEPSEEK_API_KEY`, native context window D-36) and runs the session in
`full_auto`, so the granted shell runs on the host. Not part of `make check`; everything it writes stays
under `--state-dir` and it stops the daemon it started.
"""
import argparse
import json
import os
import pathlib
import shutil
import socket as socket_module
import sqlite3
import subprocess
import sys
import time
import uuid

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

CONFIG = """# Cancel probe (A13): a real model on the native context window (D-36).
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
"""

FIRST = """Spawn exactly one worker and reply with its instance id. Do not delegate or ask it anything yet —
just report the id and end your turn without settling the goal."""

INSTRUCTION = """Run exactly this shell command in the shared workspace `{workspace}` and then report what it
printed:

{command}

Pass the command through unchanged. If it does not return on its own, say that it is still running."""


def call(bin_args: list[str], env: dict, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def protocol(socket_path: pathlib.Path, method: str, params: dict, command_id: str | None = None) -> dict:
    """One documented request/reply round trip (§9); a client per call, like the TUI's reconnect path."""
    connection = socket_module.socket(socket_module.AF_UNIX)
    connection.settimeout(30)
    connection.connect(str(socket_path))
    stream = connection.makefile("rw")
    stream.readline()  # greeting
    request = {"protocol_version": 1, "request_id": f"probe-{uuid.uuid4()}", "method": method, "params": params}
    if command_id:
        request["command_id"] = command_id
    stream.write(json.dumps(request) + "\n")
    stream.flush()
    reply = json.loads(stream.readline())
    connection.close()
    return reply


def heartbeat_lines(path: pathlib.Path) -> int:
    if not path.is_file():
        return 0
    return sum(1 for line in path.read_text().splitlines() if line.strip())


def operations(state_root: pathlib.Path) -> list[tuple[str, str, str]]:
    """(operation id, status, receipt error class) — the class says *why* it stopped."""
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    rows = []
    for operation_id, status, receipt in db.execute(
            "SELECT operation_id, status, receipt_json FROM operations ORDER BY rowid"):
        error_class = ""
        if receipt:
            try:
                error_class = (json.loads(receipt).get("error") or {}).get("class", "")
            except json.JSONDecodeError:
                error_class = "unparseable"
        rows.append((operation_id, status, error_class))
    return rows


def wait_until(predicate, timeout: float, step: float = 0.5) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(step)
    return predicate()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-cancel)")
    parser.add_argument("--stop-window", type=int, default=30, help="seconds the stop lever gets")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-cancel")
    workspace = root / "ws"
    heartbeat = workspace / "heartbeat.txt"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    common = ["--state-root", str(state_root)]
    # five ticks a second: a *single* quiet sample window can never be mistaken for "the process is
    # gone" when the tick cadence is slower than the window (D-84's rule applied to this probe)
    command = f"while true; do echo tick >> {heartbeat}; sleep 0.2; done"
    failures: list[str] = []
    daemon = None
    worker: str | None = None
    lever_pulled = False
    log = open(root / "daemon.log", "w")
    try:
        # --- the session, and a member the user hired ----------------------------
        daemon = subprocess.Popen([str(BIN), "daemon", *common, "--cwd", str(workspace), "--full-auto"],
                                  env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
        socket_path = state_root / "daemon.sock"
        if not wait_until(lambda: socket_path.exists() or daemon.poll() is not None, 30):
            failures.append("the daemon never bound its socket")
            return 1
        if not socket_path.exists():
            failures.append(f"the daemon exited: {log.read()[-400:]}")
            return 1
        print("  daemon is up")

        # --- a worker the Leader spawned (the product path for hiring one) --------
        promoted = call(["exec", *common, "--json", "--timeout", "180", "--cwd", str(workspace), FIRST], env,
                        timeout=300)
        print(f"  the Leader's spawn turn: exit={promoted.returncode}")
        listed = call(["authority", *common, "--json"], env, timeout=60)
        workers = [i["id"] for i in json.loads(listed.stdout)["instances"] if i["id"] != "i-leader"]
        if len(workers) != 1:
            report = json.loads(promoted.stdout) if promoted.stdout.strip().startswith("{") else {}
            print("  the turn's own words:", (report.get("reply") or promoted.stdout or promoted.stderr)[:300])
            failures.append(f"the Leader did not spawn exactly one worker ({workers})")
            return 1
        worker = workers[0]
        print(f"  worker {worker}")

        # a spawned worker holds no shell@workspace (§5.1/D-60); the user grants it
        granted = call(["authority", *common, "grant", "--subject", worker, "--action", "shell",
                        "--scope", "workspace", "--json"], env, timeout=60)
        print(f"  authority grant shell@workspace to {worker}: exit={granted.returncode}")
        if granted.returncode != 0:
            failures.append(f"the shell grant was refused: {granted.stdout or granted.stderr}")
            return 1

        # --- the user talks to that member directly (A21) ------------------------
        envelope = f"probe-{uuid.uuid4()}"
        submitted = protocol(socket_path, "submit_input",
                             {"instance_id": worker, "envelope_id": envelope,
                              "text": INSTRUCTION.format(workspace=workspace, command=command)},
                             f"input-{envelope}")
        if not submitted.get("ok"):
            failures.append(f"submit_input to the member was refused: {submitted.get('error')}")
            return 1

        # --- the command is really running --------------------------------------
        started = time.time()
        if not wait_until(lambda: heartbeat_lines(heartbeat) >= 2, 180):
            failures.append(f"the member never ran the command ({heartbeat_lines(heartbeat)} ticks in 180s)")
            return 1
        print(f"  the member's command is running ({heartbeat_lines(heartbeat)} ticks after "
              f"{round(time.time() - started, 1)}s)")

        # --- the user's lever ----------------------------------------------------
        ended = call(["instances", *common, "terminate", "--id", worker, "--yes", "--json"], env, timeout=60)
        print(f"  instances terminate --id {worker} --yes: exit={ended.returncode} "
              f"{(ended.stdout or ended.stderr).strip()[:300]}")
        if ended.returncode != 0:
            failures.append(f"termination was refused: {ended.stdout or ended.stderr}")
        else:
            lever_pulled = True

        stopped_at = time.time()
        mark = heartbeat_lines(heartbeat)
        quiet_after = None
        quiet_samples = 0
        while time.time() - stopped_at < args.stop_window:
            before = heartbeat_lines(heartbeat)
            time.sleep(0.5)
            quiet_samples = quiet_samples + 1 if heartbeat_lines(heartbeat) == before else 0
            if quiet_samples >= 3:  # 1.5 s of silence with a 5 Hz tick: the process group is gone
                quiet_after = time.time() - stopped_at
                break
        if quiet_after is None:
            failures.append(f"the command was still ticking {args.stop_window}s after the termination")
        else:
            print(f"  the effect stopped {round(quiet_after, 1)}s after the lever "
                  f"({mark} ticks had accumulated when it was pulled, {heartbeat_lines(heartbeat)} in total)")

        if not wait_until(lambda: all(status not in ("PREPARED", "DISPATCH_COMMITTED", "RUNNING")
                                      for _, status, _ in operations(state_root)), 30):
            failures.append(f"an operation is still running after the lever: {operations(state_root)}")
        classes = sorted({error_class for _, _, error_class in operations(state_root) if error_class})
        if "cancelled" not in classes:
            failures.append(f"the stop did not come from the user's lever (receipt classes: {classes}) — "
                            "the receipt has to name the cancellation, not the command's own timeout")
        else:
            print(f"  the operation receipt says the cancellation reached the runner (classes: {classes})")
        print(f"  operations: {operations(state_root)}")
    finally:
        # never leave a runaway command behind: if the probe failed before (or instead of) pulling the
        # lever, stop the member it created before its daemon goes away
        if not lever_pulled and worker is not None and daemon is not None and daemon.poll() is None:
            left = call(["instances", *common, "terminate", "--id", worker, "--yes", "--json"], env, timeout=60)
            if left.returncode == 0:
                print(f"  cleanup: stopped the member {worker} the probe left behind")
        if daemon is not None and daemon.poll() is None:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
        log.close()
        for failure in failures:
            print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
