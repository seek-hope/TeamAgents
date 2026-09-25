#!/usr/bin/env python3
"""The state root the CLI will refuse to open (A33/A34, live and without a model).

Both rows are about failing closed at the *file* boundary, and both had only unit-test evidence:

1. a `session.sqlite` that is **someone else's database** must be refused rather than adopted — the daemon
   passes `create = true`, and before D-87 that path happily added the whole v2 schema to a foreign file and
   stamped it, while `doctor` refused the same file. The probe builds such a root, runs `doctor` and `exec`
   against it, and then opens the file again to prove nothing was written into it;
2. a **second daemon on a live state root** must be refused (one coordinator per root), and a root whose
   daemon was killed without a goodbye must still be usable (the lock is not inherited by anything).

    make build
    python3 review/dogfood/boundary.py

It needs no credential and calls no model. Everything it writes stays under `--state-dir`.
"""
import argparse
import hashlib
import os
import pathlib
import shutil
import signal
import sqlite3
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"
CONFIG = """skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
"""


def run(bin_path: pathlib.Path, args: list[str], env: dict, timeout: int = 40) -> subprocess.CompletedProcess:
    return subprocess.run([str(bin_path), *args], env=env, capture_output=True, text=True, timeout=timeout)


def fresh_root(base: pathlib.Path, name: str) -> tuple[pathlib.Path, pathlib.Path, dict]:
    root = base / name / "root"
    workspace = base / name / "ws"
    config = base / name / "config/teamagents"
    workspace.mkdir(parents=True)
    config.mkdir(parents=True)
    (config / "config.toml").write_text(CONFIG)
    env = {**os.environ, "XDG_CONFIG_HOME": str(base / name / "config"),
           "XDG_STATE_HOME": str(base / name / "state")}
    return root, workspace, env


def foreign_database_is_refused(base: pathlib.Path, failures: list[str]) -> None:
    root, workspace, env = fresh_root(base, "foreign")
    root.mkdir(parents=True)
    db = sqlite3.connect(root / "session.sqlite")
    db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)")
    db.execute("INSERT INTO users (name) VALUES ('alice')")
    db.commit()
    db.close()
    before = hashlib.sha256((root / "session.sqlite").read_bytes()).hexdigest()

    doctor = run(BIN, ["doctor", "--state-root", str(root)], env)
    if doctor.returncode == 0:
        failures.append("doctor accepted a foreign session.sqlite")
    elif "not a v2 session database" not in doctor.stdout + doctor.stderr:
        failures.append(f"doctor's refusal does not say what is wrong: {doctor.stdout[-300:]}")
    else:
        print(f"  doctor refuses the foreign database (exit {doctor.returncode})")

    started = time.time()
    head = run(BIN, ["exec", "--state-root", str(root), "--json", "--timeout", "30", "say hi"], env)
    if head.returncode != 2:
        failures.append(f"exec on a foreign database exited {head.returncode}, expected 2 (infrastructure)")
    elif "not a v2 session database" not in head.stdout + head.stderr:
        failures.append(f"exec's refusal does not say what is wrong: {(head.stdout + head.stderr)[-300:]}")
    else:
        print(f"  exec refuses it in {round(time.time() - started, 1)}s without calling a model")
    if "users" not in head.stdout + head.stderr:
        failures.append("the refusal does not name the foreign table")

    check = sqlite3.connect(f"file:{root / 'session.sqlite'}?mode=ro", uri=True)
    names = [row[0] for row in check.execute("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")]
    rows = list(check.execute("SELECT name FROM users"))
    if names != ["users"]:
        failures.append(f"the foreign database was written to: tables are {names}")
    elif rows != [("alice",)]:
        failures.append(f"the foreign database's rows changed: {rows}")
    else:
        after = hashlib.sha256((root / "session.sqlite").read_bytes()).hexdigest()
        if after != before:
            failures.append(f"the foreign file's bytes changed ({before[:12]} -> {after[:12]})")
        else:
            print(f"  the foreign database is unchanged ({before[:12]}: its own table, its own rows)")
    # the workspace the session would have used is not the point of this probe,
    # but the root must not have grown a daemon either
    if (root / "daemon.sock").exists():
        failures.append("a daemon was started against the foreign database")


def a_second_daemon_is_refused(base: pathlib.Path, failures: list[str]) -> None:
    root, workspace, env = fresh_root(base, "coordinator")
    first = subprocess.Popen([str(BIN), "daemon", "--state-root", str(root), "--cwd", str(workspace)],
                             env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    deadline = time.time() + 30
    while time.time() < deadline and not (root / "daemon.sock").exists():
        time.sleep(0.05)
    if not (root / "daemon.sock").exists():
        failures.append("the first daemon never bound its socket")
        first.kill()
        return
    print("  the first daemon owns the root")

    second = run(BIN, ["daemon", "--state-root", str(root), "--cwd", str(workspace)], env, timeout=20)
    if second.returncode == 0:
        failures.append("a second daemon started on a live state root")
    elif "coordinator" not in second.stdout + second.stderr:
        failures.append(f"the refusal does not name the coordinator: {second.stdout + second.stderr}")
    else:
        print(f"  the second daemon is refused (exit {second.returncode})")

    # a daemon killed without a goodbye must not leave the root unusable
    first.send_signal(signal.SIGKILL)
    first.wait(timeout=10)
    third = subprocess.Popen([str(BIN), "daemon", "--state-root", str(root), "--cwd", str(workspace)],
                             env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    deadline = time.time() + 20
    restarted = False
    while time.time() < deadline:
        if third.poll() is not None:
            break
        if (root / "daemon.sock").exists():
            restarted = True
            break
        time.sleep(0.05)
    if not restarted:
        failures.append(f"the root stayed unusable after SIGKILL (daemon exit {third.poll()})")
    else:
        print("  a daemon starts again after the previous one was killed")
    third.kill()
    third.wait(timeout=10)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-boundary)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    base = pathlib.Path(args.state_dir or "/tmp/ta-boundary")
    shutil.rmtree(base, ignore_errors=True)
    base.mkdir(parents=True)
    failures: list[str] = []
    foreign_database_is_refused(base, failures)
    a_second_daemon_is_refused(base, failures)
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
