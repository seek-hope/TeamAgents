#!/usr/bin/env python3
"""Dogfood the built CLI on this repository's own evaluation fixtures.

One task at a time: copy `review/eval/r2-p6/tasks/<task>/fixture` into a scratch
directory, configure the fixture's `checks.txt` as a user-defined completion check
(`[[checks]]`, D-50), run `teamagents exec` with the fixture's prompt, then verify the
artifact by running the same acceptance command by hand. Reports the turn count, the
session's workspace and whether the model's success matches the artifact.

This is a real-model check: it needs the credential named by the profile's `api_key_env`
(DeepSeek by default) and always uses the model's native context window (D-36). It is not
part of `make check`; run it on purpose:

    python3 review/dogfood/run.py --task edit-integrity
    python3 review/dogfood/run.py --task rust-fix --state-dir /tmp/ta-dogfood

Everything lands under --state-dir (default: a fresh /tmp directory); nothing outside it is
touched, and the repository's frozen fixtures are only read.
"""
import argparse
import atexit
import json
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"
TASKS = REPO / "review/eval/r2-p6/tasks"


def toml_string(text: str) -> str:
    """A TOML literal string when it can be one, otherwise an escaped basic string."""
    return "'" + text + "'" if ("'" not in text and "\n" not in text) else json.dumps(text)


def task_files(task: str) -> tuple[pathlib.Path, str, str]:
    base = TASKS / task
    if not (base / "prompt.md").is_file() or not (base / "checks.txt").is_file():
        raise SystemExit(f"unknown task {task!r}: no prompt.md/checks.txt under {base}")
    return base / "fixture", (base / "prompt.md").read_text().strip(), (base / "checks.txt").read_text().strip()


def write_config(root: pathlib.Path, check: str, model: str) -> None:
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(
        f"""# Dogfooding probe: the current build on this repository's own fixture, with
# the fixture's acceptance command as a user-defined check (D-50). The model keeps
# its native context window (D-36); effort high follows the probe convention.
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = {toml_string(model)}
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 120
max_retries = 2
generation_options = {{ reasoning_effort = "high" }}

[[checks]]
id = "acceptance"
command = {toml_string(check)}
"""
    )


def session_summary(state_root: pathlib.Path) -> tuple[int, str, list[str]]:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    requests = list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]
    workspace = list(db.execute("SELECT workspace_ref FROM instances LIMIT 1"))[0][0]
    calls: list[str] = []
    for (message,) in db.execute("SELECT message_json FROM context_entries WHERE kind='assistant' ORDER BY idx"):
        for call in json.loads(message).get("tool_calls") or []:
            calls.append(call.get("function", {}).get("name", "?"))
    return requests, workspace, calls



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--task", required=True, help="a directory name under review/eval/r2-p6/tasks")
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-dogfood-<task>)")
    parser.add_argument("--timeout", type=int, default=900, help="exec --timeout in seconds")
    args = parser.parse_args()

    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    import os

    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    fixture, prompt, check = task_files(args.task)
    root = pathlib.Path(args.state_dir or f"/tmp/ta-dogfood-{args.task}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    atexit.register(stop_daemon, root)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    if fixture.is_dir():
        shutil.copytree(fixture, workspace, dirs_exist_ok=True)
    write_config(root, check, "deepseek-flash")

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(root / "root"), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), prompt],
        capture_output=True, text=True,
        env={**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")},
    )
    elapsed = round(time.time() - started, 1)
    print(f"task={args.task} exit={run.returncode} elapsed={elapsed}s")
    print(run.stdout.strip() or "(no report)")
    if run.stderr.strip():
        print(run.stderr.strip())

    requests, live_workspace, calls = session_summary(root / "root")
    print(f"model requests={requests} session workspace={live_workspace}")
    print(f"first tool calls={calls[:8]}")

    if live_workspace != str(workspace):
        print(f"FAIL: the session worked in {live_workspace}, not in {workspace}")
        return 1
    verify = subprocess.run(["bash", "-lc", check], cwd=workspace, capture_output=True, text=True)
    print(f"acceptance check (independent) exit={verify.returncode}")
    print((verify.stdout + verify.stderr).strip()[:600])
    if verify.returncode != 0:
        print("FAIL: the artifact does not satisfy the fixture's own check")
        return 1
    print("ok: the artifact satisfies the fixture's check")
    return 0


if __name__ == "__main__":
    sys.exit(main())
