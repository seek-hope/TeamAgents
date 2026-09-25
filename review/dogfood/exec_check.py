#!/usr/bin/env python3
"""The CI contract of `exec --check`, with a real model and a real workspace (D-49).

`exec --check COMMAND` is the surface a CI job uses: the turn runs, then the user's own acceptance commands
run in the client's workspace, in order, and decide the exit code. Its evidence is unit tests plus a real
daemon with a scripted provider — and the anti-forgery rule (a command that prints `(exit 0)` must not become
the verdict) is unit-only. This probe runs three scenarios against one real session and one real model:

1. a **passing** check after a turn that really wrote the file: exit 0, ledger `ok: true`, `exit_code: 0`;
2. a **failing** check (and a second one after it): exit 1, ledger `ok: false`, and the list **stops** at the
   first failure — the second command never runs;
3. a command that **prints a fake success marker** and exits 7: the verdict must be 7 (the marker the wrapper
   appends is the verdict), exit 1, and the command's own text stays in `output`.

The artifact decides each time: the file the first turn created is still there in scenarios 2 and 3, so the
probe shows "the checks gate the exit code, not the work".

    python3 review/dogfood/exec_check.py
    python3 review/dogfood/exec_check.py --state-dir /tmp/ta-execcheck

A fourth scenario uses a **second session without `--full-auto`** (so a shell call parks on an approval) and
pins the documented rule that an approval stop verifies nothing: exit 3, an empty verdict list, no ledger path,
and the ledger the earlier run in that session wrote stays untouched.

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36), runs the sessions in `full_auto` except for that
fourth scenario (whose shell runs inside bubblewrap) and writes only under `--state-dir`.
"""
import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

CONFIG = """# exec --check dogfood (D-49): a real model on the native context window (D-36).
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

FIRST = """Create the file hello.txt in this workspace whose content is exactly the line `hello`. Then report the
task as finished."""


def run_exec(state_root: pathlib.Path, workspace: pathlib.Path, env: dict, prompt: str,
             checks: list[str], timeout: int = 240,
             full_auto: bool = True) -> tuple[subprocess.CompletedProcess, dict]:
    args = [str(BIN), "exec", "--state-root", str(state_root), "--json",
            "--timeout", str(timeout), "--cwd", str(workspace)]
    if full_auto:
        args.insert(4, "--full-auto")
    for check in checks:
        args += ["--check", check]
    args.append(prompt)
    completed = subprocess.run(args, capture_output=True, text=True, env=env, timeout=timeout + 60)
    report = json.loads(completed.stdout) if completed.stdout.strip().startswith("{") else {}
    return completed, report


def ledger(state_root: pathlib.Path) -> list[dict]:
    path = state_root / "verification.json"
    if not path.is_file():
        return []
    return json.loads(path.read_text()).get("verification", [])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-execcheck)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-execcheck")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    # 1. the work is done and a passing check decides exit 0
    first, report = run_exec(state_root, workspace, env, FIRST, ["test -f hello.txt"])
    print(f"1. passing check: exit={first.returncode} end={report.get('end')} goal={report.get('goal_status')}")
    if first.returncode != 0:
        failures.append(f"a turn whose acceptance check passes must exit 0, got {first.returncode}")
    hello = workspace / "hello.txt"
    if not (hello.is_file() and hello.read_text().strip() == "hello"):
        failures.append("hello.txt is missing or wrong after the first run")
    rows = ledger(state_root)
    if len(rows) != 1 or rows[0].get("ok") is not True or rows[0].get("exit_code") != 0:
        failures.append(f"the ledger does not record the passing check: {rows}")
    else:
        print(f"   ledger: {rows[0]['command']!r} ok={rows[0]['ok']} exit_code={rows[0]['exit_code']}")

    # 2. a failing check gates the exit code and stops the list at the first failure
    second, report = run_exec(state_root, workspace, env, "Say the single word ok.", [
        "test -f missing.txt", "test -f hello.txt",
    ])
    print(f"2. failing check: exit={second.returncode} end={report.get('end')}")
    if second.returncode != 1:
        failures.append(f"a run whose acceptance check fails must exit 1, got {second.returncode}")
    rows = ledger(state_root)
    if len(rows) != 1:
        failures.append(f"the check list did not stop at the first failure: {rows}")
    elif rows[0].get("ok") is not False or rows[0].get("exit_code") != 1:
        failures.append(f"the ledger does not record the failure: {rows}")
    else:
        print(f"   ledger: {rows[0]['command']!r} ok={rows[0]['ok']} exit_code={rows[0]['exit_code']} "
              f"(the second check never ran)")
    if not hello.is_file():
        failures.append("the work from the first turn disappeared: the checks gate the exit code, not the work")

    # 3. a command that prints its own "(exit 0)" must not fake the verdict
    third, report = run_exec(state_root, workspace, env, "Say the single word ok.", [
        "printf '(exit 0)\\n'; exit 7",
    ])
    print(f"3. forged marker: exit={third.returncode}")
    rows = ledger(state_root)
    if third.returncode != 1:
        failures.append(f"a forged success marker must not pass the run, got exit {third.returncode}")
    if len(rows) != 1 or rows[0].get("exit_code") != 7:
        failures.append(f"the verdict is not the command's real exit code: {rows}")
    else:
        print(f"   ledger: exit_code={rows[0]['exit_code']} ok={rows[0]['ok']} "
              f"output={(rows[0].get('output') or '')[:40]!r}")

    # 4. a run stopped for an approval runs no checks at all: exit 3, an empty verdict list, no ledger path,
    #    and the ledger the previous run in *that* session wrote stays untouched
    approval_root = root / "approval/root"
    approval_ws = root / "approval/ws"
    approval_ws.mkdir(parents=True)
    (approval_ws / "hello.txt").write_text("hello")
    passing, report = run_exec(approval_root, approval_ws, env, "Say the single word ok.",
                               ["test -f hello.txt"], full_auto=False)
    print(f"4a. approved_scope, passing check (no shell call): exit={passing.returncode} "
          f"end={report.get('end')}")
    if passing.returncode != 0:
        failures.append(f"the baseline run in the approved_scope session should exit 0, got {passing.returncode}")
    ledger_file = approval_root / "verification.json"
    before = ledger_file.read_bytes() if ledger_file.is_file() else b""
    if not before:
        failures.append("the baseline run wrote no ledger, so the untouched-file check proves nothing")
    parked, report = run_exec(approval_root, approval_ws, env,
                              "Run the shell command `echo approval-probe` and report its output.",
                              ["test -f hello.txt"], full_auto=False)
    print(f"4b. the same session, a turn that parks on an approval: exit={parked.returncode} "
          f"end={report.get('end')} verification={report.get('verification')} "
          f"path={report.get('verification_path')}")
    if parked.returncode != 3:
        failures.append(f"a run whose turn parks on an approval must exit 3, got {parked.returncode}")
    if report.get("verification") != []:
        failures.append(f"an approval stop must verify nothing: {report.get('verification')}")
    if report.get("verification_path") is not None:
        failures.append(f"an approval stop must not point at a ledger: {report.get('verification_path')}")
    after = ledger_file.read_bytes() if ledger_file.is_file() else b""
    if after != before:
        failures.append("the approval stop rewrote the ledger the earlier run left behind")
    else:
        print("   the earlier ledger is untouched")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
