#!/usr/bin/env python3
"""The user's own policy hook, live: a veto that really vetoes, and a broken hook that must not (D-92).

`[hooks]` is the surface where *your* programs run around the runtime's own work — `notify` on every event,
`pre_tool` in front of every native tool call. Its whole contract (exit 0 allows, exit 2 denies with the
first stderr line as the reason, anything else allows, a hanging hook is killed) is covered by unit tests in
`engine/src/hooks.rs`; nothing had driven it with a real model, a real daemon and real tool calls.

    python3 review/dogfood/hooks.py                 # the veto must deny the shell call
    python3 review/dogfood/hooks.py --mode broken   # negative control: exit 1 must NOT deny

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36) and writes only under `--state-dir`. Two scripts
are generated there: `notify.sh` records `argv[1]` and the JSON payload it reads on stdin, `veto.sh` records
the payload it is asked about and denies the `shell` tool by exit code. The probe then asks a real model for
one file write and one shell command and checks: the write happened, the shell command did not, the model was
told the reason, the hook saw the real payloads, and the notify stream carries real events with real ids.
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import sqlite3
import stat
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
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

PROMPT = """Do two things in this workspace, in this order:
1. create the file kept.txt whose content is exactly `kept`;
2. run the shell command `echo veto-me > vetoed.txt`.
Then report what happened with each of them."""

NOTIFY = """#!/bin/sh
# the user's notify hook: argv[1] is the event name, the payload arrives on stdin
event="$1"
payload=$(cat)
printf '%s %s\\n' "$event" "$payload" >> {log}
exit 0
"""

# exit code 2 denies; the first stderr line is the reason the model is given
VETO = """#!/bin/sh
payload=$(cat)
printf '%s\\n' "$payload" >> {log}
if printf '%s' "$payload" | grep -Eq '"tool" *: *"shell"'; then
  echo "probe rule: shell commands are not allowed in this project" >&2
  exit {code}
fi
exit 0
"""


def call(bin_args: list[str], env: dict, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def log_lines(path: pathlib.Path) -> list[str]:
    return path.read_text().splitlines() if path.is_file() else []


def entries(state_root: pathlib.Path) -> list[str]:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    return [row[0] for row in db.execute("SELECT message_json FROM context_entries ORDER BY idx")]


def write_script(path: pathlib.Path, body: str) -> None:
    path.write_text(body)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    leak_guard.stop_daemons(state_root)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", default="veto", choices=["veto", "broken"])
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-hooks)")
    parser.add_argument("--timeout", type=int, default=300, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-hooks")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    notify_log = root / "notify.log"
    pre_tool_log = root / "pre_tool.log"
    notify = root / "notify.sh"
    veto = root / "veto.sh"
    write_script(notify, NOTIFY.format(log=notify_log))
    write_script(veto, VETO.format(log=pre_tool_log, code=2 if args.mode == "veto" else 1))
    (root / "config/teamagents/config.toml").write_text(
        "# Hooks dogfood (D-92): the user's own policy and notification programs.\n"
        "skills_paths = []\n\n" + MODEL + f'\n[hooks]\nnotify = ["{notify}"]\npre_tool = ["{veto}"]\n')
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    run = call(["exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout",
                str(args.timeout), "--cwd", str(workspace), PROMPT], env, timeout=args.timeout + 60)
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"mode={args.mode} exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} "
          f"goal={report.get('goal_status')}")

    # the hook saw the calls it is asked about, with the real arguments
    saw = log_lines(pre_tool_log)
    shell_payloads = [line for line in saw if '"tool":"shell"' in line.replace(" ", "")]
    if not saw:
        failures.append("the pre_tool hook was never called")
    else:
        print(f"  the pre_tool hook was consulted {len(saw)} times "
              f"({len(shell_payloads)} of them about shell)")
    if not shell_payloads:
        failures.append("the pre_tool hook never saw the shell call, so the veto could not be exercised")

    # the file tool ran; the shell tool did or did not, depending on the mode
    kept = workspace / "kept.txt"
    vetoed = workspace / "vetoed.txt"
    if kept.is_file() and kept.read_text().strip() == "kept":
        print("  kept.txt exists: the hook allowed the file write")
    else:
        failures.append(f"kept.txt is missing or wrong: {kept.read_text() if kept.is_file() else '(absent)'!r}")
    conversation = entries(state_root)
    denied = [entry for entry in conversation if "denied by pre_tool hook" in entry]
    if args.mode == "veto":
        if vetoed.exists():
            failures.append("the shell command ran although the policy hook denied it")
        else:
            print("  vetoed.txt does not exist: the denied command never ran")
        if not denied:
            failures.append("the model was never told the call was denied")
        else:
            print(f"  the model was told: {denied[-1][:160]}")
        if not any("probe rule" in entry for entry in conversation):
            failures.append("the hook's own reason did not reach the model")
    else:
        if vetoed.is_file() and vetoed.read_text().strip() == "veto-me":
            print("  vetoed.txt exists: a hook that exits 1 did NOT deny the call (the documented rule)")
        else:
            failures.append(f"a broken hook blocked the call: vetoed.txt is "
                            f"{vetoed.read_text() if vetoed.is_file() else '(absent)'!r}")
        if denied:
            failures.append("a hook that exits 1 was treated as a denial")

    # the notify stream: real events, real ids, parseable payloads
    notify_lines = log_lines(notify_log)
    events = []
    for line in notify_lines:
        event, _, payload = line.partition(" ")
        try:
            body = json.loads(payload)
        except json.JSONDecodeError:
            failures.append(f"a notify payload was not JSON: {line[:120]}")
            continue
        events.append((event, body))
    kinds = {}
    for event, body in events:
        kinds[event] = kinds.get(event, 0) + 1
        if body.get("session_id") != "s-main" or not body.get("payload", {}).get("instance_id"):
            failures.append(f"an event payload lacks the session/instance identity: {body}")
            break
    print(f"  notify received {len(events)} events {kinds}")
    if kinds.get("tool_call", 0) < 2:
        failures.append(f"the notify hook did not see the tool calls: {kinds}")
    if not kinds.get("run_completed"):
        failures.append(f"the notify hook did not see the turn completing: {kinds}")
    calls = [body["payload"] for event, body in events if event == "tool_call"]
    tools = sorted({call.get("tool", "?") for call in calls})
    if args.mode == "veto" and "shell" not in tools:
        failures.append(f"the notify stream does not mention the shell call: {tools}")
    elif args.mode == "veto":
        print(f"  the notify stream names the tools it saw: {tools}")
    if args.mode == "veto" and not any(call.get("ok") is False for call in calls):
        failures.append("no tool call was reported as failed to the notify hook")
    if run.returncode == 124:
        failures.append("the turn hit its deadline (a hook must never wedge a turn)")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
