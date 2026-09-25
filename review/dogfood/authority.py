#!/usr/bin/env python3
"""A real-model check of the user's authority surface (D-61) end to end.

One session, two turns: the Leader spawns a worker and asks it whether it can run shell commands
in the shared workspace (it cannot: a spawned worker holds no `shell@workspace`, §5.1/D-60, so the
tool is not even offered to it). The user then grants the worker the capability through
`teamagents authority grant`, sends a second instruction, and the same worker runs a shell command.

The artifact decides: that command writes `proof.txt` into the shared workspace, so the file must be
absent after the first turn and present after the second. The probe also records the worker id the
grant was addressed to, the revocation (`authority revoke`) and that the capability disappears again.

Two things it guards against, because both were observed here: a model that answers with prose while
its task stays open is re-asked by the runtime (unbounded without a goal budget), so the probe parks
such a worker and reports it; and the same session shape (a turn that ends on an accepted `finish`,
then a new task) is what produced D-62's wire error, which the first run of this probe found.

    python3 review/dogfood/authority.py                    # fresh /tmp state root
    python3 review/dogfood/authority.py --state-dir /tmp/ta-authority

It is a real-model check: it needs `DEEPSEEK_API_KEY` and uses the model's native context window
(D-36). It is not part of `make check`; everything it writes stays under `--state-dir`.
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

CONFIG = """# Authority probe (D-61): a real model on the native context window (D-36).
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

FIRST = """Spawn one worker, and delegate this task to it: say in one sentence whether you can run shell
commands in the shared workspace right now, and which tools you have for that. Report the worker's answer
to me, then wait for my next message — do not finish or settle the goal yet."""

SECOND = """I have granted that worker shell access to the shared workspace. Ask the same worker to run the
shell command `printf granted > {proof}` in that workspace and report the command's output."""


def call(bin_args: list[str], env: dict, cwd: pathlib.Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, cwd=cwd)


def turns(state_root: pathlib.Path) -> int:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    return list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]


def session_state(state_root: pathlib.Path, worker: str) -> tuple[int, str]:
    """(model requests so far, the worker's open task or "none").

    A turn whose model answers with prose while its task stays open is re-asked by the runtime (see the
    known gap in docs/ACCEPTANCE.md), which is unbounded without a goal budget. A probe must notice that
    instead of letting it run, so the caller parks the instance and reports the finding.
    """
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    handled = list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]
    open_tasks = list(
        db.execute("SELECT status FROM tasks WHERE assignee = ?1 AND status IN ('PENDING', 'RUNNING')", [worker])
    )
    return handled, (open_tasks[0][0] if open_tasks else "none")


def park(socket: pathlib.Path, instance: str, reason: str) -> str:
    """Park an instance through the daemon protocol, the way the TUI does.

    There is no CLI verb for it (user-side pause/resume is a TUI action), so the probe speaks the
    documented socket protocol (`protocol_version` 1) directly.
    """
    import socket as socket_module

    try:
        connection = socket_module.socket(socket_module.AF_UNIX, socket_module.SOCK_STREAM)
        connection.settimeout(10)
        connection.connect(str(socket))
        stream = connection.makefile("rw")
        stream.readline()  # greeting
        stream.write(
            json.dumps(
                {
                    "protocol_version": 1,
                    "request_id": "probe-park",
                    "command_id": "probe-park",
                    "method": "set_lifecycle",
                    "params": {"instance_id": instance, "lifecycle": "PARKED", "reason": reason},
                }
            )
            + "\n"
        )
        stream.flush()
        reply = json.loads(stream.readline())
        return "parked" if reply.get("ok") else f"refused: {reply.get('error')}"
    except OSError as error:  # the daemon may already be gone
        return f"could not park {instance}: {error}"



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-authority)")
    parser.add_argument("--timeout", type=int, default=600, help="exec --timeout in seconds")
    parser.add_argument("--budget", type=int, default=24, help="model requests one phase may spend before the probe stops the loop")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-authority")
    workspace = root / "ws"
    proof = workspace / "proof.txt"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    common = ["--state-root", str(state_root), "--full-auto"]
    failures: list[str] = []

    # --- turn 1: the worker cannot run the command -------------------------------
    started = time.time()
    first = call(["exec", *common, "--json", "--timeout", str(args.timeout), "--cwd", str(workspace),
                  FIRST.format(proof=proof)], env)
    print(f"turn 1: exit={first.returncode} elapsed={round(time.time() - started, 1)}s")
    report = json.loads(first.stdout or "{}") if first.stdout.strip().startswith("{") else {}
    print("  reply:", (report.get("reply") or first.stdout.strip() or first.stderr.strip())[:400])
    if proof.exists():
        failures.append("the worker wrote the file before any grant: the boundary is not real")
    else:
        print("  proof.txt absent before the grant (as expected)")

    # --- the user grants the worker the shared-workspace shell -------------------
    listed = call(["authority", *common, "--json"], env)
    view = json.loads(listed.stdout)
    workers = [i["id"] for i in view["instances"] if i["id"] != "i-leader"]
    if len(workers) != 1:
        failures.append(f"expected exactly one worker, saw {workers}")
        print("FAIL:", failures[-1])
        return 1
    worker = workers[0]
    handled, open_task = session_state(state_root, worker)
    if open_task != "none" and handled > args.budget:
        print(
            f"note: the worker still owes task {open_task} after {handled} model requests — the runtime "
            "keeps asking a model that answers with prose (docs/ACCEPTANCE.md, known gap). "
            f"{park(root / 'root/daemon.sock', worker, 'probe stopped a re-asking loop')}. "
            "The authority commands still run below; the worker's turn does not."
        )
        failures.append(
            f"the worker spun: {handled} model requests with task {open_task} open (see the known gap)"
        )
    granted = call(["authority", *common, "grant", "--subject", worker, "--action", "shell",
                    "--scope", "workspace", "--json"], env)
    print(f"grant: exit={granted.returncode} {granted.stdout.strip() or granted.stderr.strip()}")
    if granted.returncode != 0:
        failures.append("the grant was refused")
    if granted.stderr.strip():
        print("  note:", granted.stderr.strip())
    grant_id = (json.loads(granted.stdout) if granted.stdout.strip() else {}).get("grant_id", "")

    # --- turn 2: the same worker runs it -----------------------------------------
    started = time.time()
    second = call(["exec", *common, "--json", "--timeout", str(args.timeout), "--cwd", str(workspace),
                   SECOND.format(proof=proof)], env)
    print(f"turn 2: exit={second.returncode} elapsed={round(time.time() - started, 1)}s")
    report = json.loads(second.stdout or "{}") if second.stdout.strip().startswith("{") else {}
    print("  reply:", (report.get("reply") or second.stdout.strip() or second.stderr.strip())[:400])
    if proof.is_file() and proof.read_text().strip() == "granted":
        print("  proof.txt present: the worker ran the shell command after the grant")
    else:
        failures.append("the worker still could not run the command after the grant")

    # --- revoking takes it away again -------------------------------------------
    revoked = call(["authority", *common, "revoke", "--grant", grant_id[:12], "--json"], env)
    print(f"revoke: exit={revoked.returncode} {revoked.stdout.strip() or revoked.stderr.strip()}")
    if revoked.returncode != 0:
        failures.append("the revocation was refused")
    after = json.loads(call(["authority", *common, "--json"], env).stdout)
    live = [g for g in after["grants"]
            if g["subject"] == worker and g["action"] == "shell" and not g["revoked"]]
    if live:
        failures.append("the revoked shell grant is still live")
    else:
        print("revoked: the worker holds no live shell grant")

    print(f"model requests in the session: {turns(state_root)}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
