#!/usr/bin/env python3
"""`instruction_files` today: declared, validated, and **not read into a prompt** (D-102).

The user config has an `instruction_files` list, the loader validates that each path exists, and `doctor` used
to print

    [ok  ] instruction files        1 file(s) reach every member's prompt

while nothing in this build reads those files: a member's system text comes from its profile
(`core/src/kernel/instance.rs` pushes `profile.instructions` as the system message), and the only readers of
the key are the loader, the validator and that doctor row. This probe pins the current truth so the promise
cannot come back silently — it checks that the file really holds the canary, that the prompt exists and is
inspectable (the leader profile carries the Leader text), and that the canary is **not** in it — and it also
requires `doctor` to say "not applied".

When the feature lands, the third check flips to "the canary is in the prompt" and this probe becomes its
acceptance test; until then the row tells the user the truth instead of promising a prompt they never get.

    python3 review/dogfood/instructions.py

It needs `DEEPSEEK_API_KEY`, uses the native window (D-36) and writes only under `--state-dir`.
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
CANARY = "CANARY_INSTRUCTION_9F2A"

CONFIG = """# Instruction-file dogfood (D-102): a real model on the native context window (D-36).
skills_paths = []
instruction_files = ["__RULES__"]

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


def call(bin_args: list[str], env: dict, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-instructions)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-instructions")
    workspace = root / "ws"
    rules = root / "AGENTS.md"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    rules.write_text(f"# Project instructions\n{CANARY}: always answer with the single word canary.\n")
    (root / "config/teamagents/config.toml").write_text(CONFIG.replace("__RULES__", str(rules)))
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    # 1. the file is really there and really holds the canary
    if CANARY not in rules.read_text():
        failures.append("the probe's own instruction file lost its canary")
        return 1
    print(f"  the instruction file holds the canary ({rules})")

    # 2. doctor tells the user what is true
    doctor = call(["doctor", "--state-root", str(state_root)], env)
    row = next((line for line in doctor.stdout.splitlines() if "instruction files" in line), "")
    print(f"  doctor: {row.strip()[:150]}")
    if "[WARN]" not in row or "not applied" not in row:
        failures.append(f"doctor still promises the files reach a prompt: {row.strip()!r}")

    # 3. one turn, then look at the prompt the model was given
    started = time.time()
    run = call(["exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "120",
                "--cwd", str(workspace), "Say the single word ok."], env, timeout=200)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"  one turn: exit={run.returncode} end={report.get('end')} ({round(time.time() - started, 1)}s)")
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    row = db.execute("SELECT profile_json FROM instances WHERE id = 'i-leader'").fetchone()
    if row is None:
        failures.append("the session has no leader profile to inspect")
        return 1
    instructions = json.loads(row[0]).get("instructions", "")
    if not instructions:
        failures.append("the leader profile carries no instructions at all, so the check proves nothing")
    else:
        print(f"  the leader prompt is inspectable ({len(instructions)} chars, {instructions[:40]!r}…)")
    if CANARY in instructions:
        # this is the check that flips when the feature lands
        failures.append("the canary is in the prompt: `instruction_files` now works — flip this probe's "
                        "expectation and record the decision")
    else:
        print("  the canary is not in the prompt: declared, not applied (D-102)")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
