#!/usr/bin/env python3
"""What the user's lifecycle levers do to a run that is already in flight (D-82/D-98, A13/A21).

Two levers, two different truths, and only one of them ends the run:

* **terminate** (`instances terminate --id … --yes`) closes the instance's open execution — nothing will ever
  answer the run. Before D-98 the client waited out its own deadline and answered `end: "timeout"` (`124`),
  which says "still running" about an instance that is gone. Now the run ends at once with the truth.
* **reset** (the `reset_instance` command — no CLI verb, so the probe speaks the documented protocol) closes
  the epoch's execution and moves the instance to a new epoch: the turn this run was waiting on is gone, so the
  run must end at once and say why (before D-99's sibling fix it waited out its deadline and reported
  `timeout`).
* **pause** (`instances pause --id …`) stops the instance at a *boundary*: the turn in flight may still
  finish, and a resumed instance continues the work — so a run keeps following its own turn, and if it wins
  the race with the caller's deadline it reports the turn's outcome (measured: `end=completed`, exit 0).
  The probe drives exactly that: pause mid-run, resume, and let the run finish.

    python3 review/dogfood/lifecycle_run.py                     # the terminate lever
    python3 review/dogfood/lifecycle_run.py --lever pause       # pause + resume (the run still finishes)

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36) and writes only under `--state-dir`.
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

CONFIG = """# Lifecycle-lever dogfood (D-98): a real model on the native context window (D-36).
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
"""

TERMINATE_PROMPT = "Write a 600-word essay about tides. Then report the task as finished."
PAUSE_PROMPT = ("Write the file tides.txt in this workspace with a single sentence about tides, then report "
                "the task as finished.")


def protocol(socket_path: pathlib.Path, method: str, params: dict, command_id: str | None = None) -> dict:
    """One documented request/reply round trip (§9) — a `reset_instance` has no CLI verb."""
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


def call(bin_args: list[str], env: dict, timeout: int = 60) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def instance_row(state_root: pathlib.Path, env: dict, instance: str = "i-leader") -> dict:
    listed = call(["instances", "--state-root", str(state_root), "--json"], env)
    if not listed.stdout.strip().startswith("{"):
        return {}
    return next((row for row in json.loads(listed.stdout)["instances"] if row["id"] == instance), {})


def session(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    instances = list(db.execute("SELECT id, lifecycle, phase FROM instances"))
    requests = list(db.execute("SELECT request_id, status FROM model_requests"))
    goals = list(db.execute("SELECT id, status FROM goals"))
    events = [row[0] for row in db.execute("SELECT kind FROM events ORDER BY sequence")]
    return {"instances": instances, "requests": requests, "goals": goals, "events": events}


def wait_until(predicate, timeout: float, step: float = 0.25) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(step)
    return predicate()


def spawn_run(env: dict, state_root: pathlib.Path, workspace: pathlib.Path, prompt: str, timeout: int,
              check: str | None) -> subprocess.Popen:
    args = [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", str(timeout),
            "--cwd", str(workspace)]
    if check:
        args += ["--check", check]
    args.append(prompt)
    return subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def wait_in_flight(state_root: pathlib.Path, env: dict, timeout: float = 90) -> str:
    wait_until(lambda: instance_row(state_root, env).get("phase") in ("MODEL_PENDING", "TOOLS_PENDING"), timeout)
    return instance_row(state_root, env).get("phase", "?")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lever", default="terminate", choices=["terminate", "pause", "reset"])
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-lifecycle)")
    parser.add_argument("--timeout", type=int, default=180, help="exec --timeout for the paused run")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-lifecycle")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []
    running = None
    try:
        if args.lever == "terminate":
            marker = workspace / "check-ran"
            running = spawn_run(env, state_root, workspace, TERMINATE_PROMPT, 120, f"touch {marker}")
            phase = wait_in_flight(state_root, env)
            if phase not in ("MODEL_PENDING", "TOOLS_PENDING"):
                failures.append(f"the run never reached the model (phase {phase})")
                return 1
            print(f"  the run is in flight (leader phase {phase})")
            started = time.time()
            lever = call(["instances", "terminate", "--state-root", str(state_root), "--id", "i-leader",
                          "--yes", "--json"], env)
            print(f"  instances terminate --id i-leader --yes: exit={lever.returncode}")
            if lever.returncode != 0:
                failures.append(f"termination was refused: {lever.stdout or lever.stderr}")
            out, _ = running.communicate(timeout=180)
            elapsed = round(time.time() - started, 1)
            report = json.loads(out) if out.strip().startswith("{") else {}
            print(f"  the run ended {elapsed}s after the lever: exit={running.returncode} "
                  f"end={report.get('end')} failure={report.get('failure')!r}")
            if running.returncode != 1:
                failures.append(f"a terminated run must exit 1, got {running.returncode}")
            if report.get("end") != "failed":
                failures.append(f"the termination is this run's outcome, not {report.get('end')!r}")
            failure = report.get("failure") or ""
            if "is terminated" not in failure:
                failures.append(f"the failure does not name the termination: {failure!r}")
            if "termination is final" not in failure:
                failures.append(f"the failure does not carry the advice: {failure!r}")
            if elapsed > 20:
                failures.append(f"the run waited {elapsed}s after the lever instead of ending at once")
            if marker.exists():
                failures.append("an acceptance command ran for a run whose turn never finished")
            facts = session(state_root)
            if not any(row[1] == "TERMINATED" for row in facts["instances"]):
                failures.append(f"the instance is not terminated: {facts['instances']}")
            else:
                print("  the session shows the instance TERMINATED")
            if any(row[1] == "PENDING" for row in facts["requests"]):
                failures.append(f"the in-flight request was left open: {facts['requests']}")
            else:
                print(f"  the in-flight request is closed ({facts['requests']})")
        elif args.lever == "reset":
            running = spawn_run(env, state_root, workspace, TERMINATE_PROMPT, 120, None)
            phase = wait_in_flight(state_root, env)
            if phase not in ("MODEL_PENDING", "TOOLS_PENDING"):
                failures.append(f"the run never reached the model (phase {phase})")
                return 1
            print(f"  the run is in flight (leader phase {phase})")
            began = time.time()
            reset = protocol(state_root / "daemon.sock", "reset_instance",
                             {"instance_id": "i-leader", "reason": "probe reset"}, "probe-reset")
            if not reset.get("ok"):
                failures.append(f"reset_instance was refused: {reset.get('error')}")
            else:
                print(f"  reset_instance: {reset['result']}")
            out, _ = running.communicate(timeout=180)
            elapsed = round(time.time() - began, 1)
            report = json.loads(out) if out.strip().startswith("{") else {}
            print(f"  the run ended {elapsed}s after the reset: exit={running.returncode} "
                  f"end={report.get('end')} failure={report.get('failure')!r}")
            if running.returncode != 1:
                failures.append(f"a reset run must exit 1, got {running.returncode}")
            if report.get("end") != "failed":
                failures.append(f"the reset is this run's outcome, not {report.get('end')!r}")
            failure = report.get("failure") or ""
            if "was reset while this run was in flight" not in failure or "epoch" not in failure:
                failures.append(f"the failure does not name the reset and the epoch move: {failure!r}")
            if elapsed > 20:
                failures.append(f"the run waited {elapsed}s after the reset instead of ending at once")
            facts = session(state_root)
            if "instance_reset" not in facts["events"]:
                failures.append("the session did not record the reset")
            else:
                print("  the session recorded the reset")
            if not any(row[1] == "ACTIVE" for row in facts["instances"]):
                failures.append(f"the instance is not ACTIVE after the reset: {facts['instances']}")
            if any(row[1] == "PENDING" for row in facts["requests"]):
                failures.append(f"the old epoch's request was left open: {facts['requests']}")
        else:
            # pause stops the instance at a boundary; resume lets the *same run* finish the work
            running = spawn_run(env, state_root, workspace, PAUSE_PROMPT, args.timeout, None)
            phase = wait_in_flight(state_root, env)
            if phase not in ("MODEL_PENDING", "TOOLS_PENDING"):
                failures.append(f"the run never reached the model (phase {phase})")
                return 1
            print(f"  the run is in flight (leader phase {phase})")
            paused = call(["instances", "pause", "--state-root", str(state_root), "--id", "i-leader"], env)
            waited = wait_until(lambda: instance_row(state_root, env).get("lifecycle") == "PAUSED", 20)
            print(f"  instances pause: exit={paused.returncode}; the instance is "
                  f"{instance_row(state_root, env).get('lifecycle')}")
            if not waited:
                failures.append("the pause did not reach the daemon")
            time.sleep(5.0)
            if running.poll() is not None:
                failures.append("the run ended at the pause instead of following its turn")
            resumed = call(["instances", "resume", "--state-root", str(state_root), "--id", "i-leader"], env)
            print(f"  instances resume: exit={resumed.returncode}")
            out, _ = running.communicate(timeout=args.timeout + 60)
            report = json.loads(out) if out.strip().startswith("{") else {}
            print(f"  the run ended: exit={running.returncode} end={report.get('end')} "
                  f"goal={report.get('goal_status')} reply={(report.get('reply') or '')[:60]!r}")
            if running.returncode != 0:
                failures.append(f"a resumed run must finish its own turn: exit {running.returncode}")
            if report.get("end") not in ("completed", "reply"):
                failures.append(f"the resumed run did not report its turn's outcome: {report.get('end')!r}")
            tides = workspace / "tides.txt"
            if not tides.is_file():
                failures.append("the work the resumed turn was asked for is missing")
            facts = session(state_root)
            if "instance_lifecycle" not in facts["events"]:
                failures.append("the pause was not recorded")
            if not any(row[1] == "SUCCEEDED" for row in facts["goals"]):
                failures.append(f"the goal did not settle after the resume: {facts['goals']}")
            else:
                print("  the goal settled SUCCEEDED after the resume, and the run reported it")
    finally:
        if running is not None and running.poll() is None:
            running.kill()
        subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
