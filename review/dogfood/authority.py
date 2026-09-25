#!/usr/bin/env python3
"""A real-model check of the user's authority surface (D-61) end to end.

One session, two turns: the Leader spawns a worker and delegates a shell command to it, and
the worker *cannot* run it — a spawned worker holds no `shell@workspace` (§5.1), so the tool is
not even offered to it (D-60). The user then grants the worker the capability through
`teamagents authority grant`, sends a second instruction, and the same worker runs the command.

The artifact decides: the command writes `proof.txt` into the shared workspace, so the file must
be absent after the first turn and present after the second. The probe also records the worker
id the grant was addressed to, the grant's revocation (`authority revoke`), and that the
capability disappears again.

    python3 review/dogfood/authority.py                    # fresh /tmp state root
    python3 review/dogfood/authority.py --state-dir /tmp/ta-authority

It is a real-model check: it needs `DEEPSEEK_API_KEY` and uses the model's native context window
(D-36). It is not part of `make check`; everything it writes stays under `--state-dir`.
"""
import argparse
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

FIRST = """Spawn one worker, and delegate this task to it: run the shell command
`printf granted > {proof}` in the shared workspace, read the command's output back, and report
whether the command ran. Then tell me what the worker said and wait for my next message — do not
finish or settle the goal yet."""

SECOND = """I have granted that worker shell access to the shared workspace. Ask the same worker to run
the same command again (`printf granted > {proof}`) and report the output."""


def call(bin_args: list[str], env: dict, cwd: pathlib.Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, cwd=cwd)


def turns(state_root: pathlib.Path) -> int:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    return list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-authority)")
    parser.add_argument("--timeout", type=int, default=600, help="exec --timeout in seconds")
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
        print("  proof.txt absent (as expected without a grant)")

    # --- the user grants the worker the shared-workspace shell -------------------
    listed = call(["authority", *common, "--json"], env)
    view = json.loads(listed.stdout)
    workers = [i["id"] for i in view["instances"] if i["id"] != "i-leader"]
    if len(workers) != 1:
        failures.append(f"expected exactly one worker, saw {workers}")
        print("FAIL:", failures[-1])
        return 1
    worker = workers[0]
    print(f"authority: subject={worker} revision={view['revision']}")
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
