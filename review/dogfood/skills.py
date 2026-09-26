#!/usr/bin/env python3
"""A real-model check that the configured Skills registry reaches the model (A26/D-34).

The probe writes a skills root with one skill whose *body* carries a token generated for
this run (the token is deliberately absent from the YAML description, so a model that only
searched cannot know it), points `skills_paths` at that root, and asks the model to read the
skill and follow it:

    python3 review/dogfood/skills.py                    # DeepSeek
    python3 review/dogfood/skills.py --provider kimi    # over `responses`

It asserts what the design promises about skills: they are *searched and read on demand*
(the `skill` tool call and its receipt are in the conversation, and the receipt carries the
body, not the blurb), the instructions then reach the model (the file the skill asks for
exists with the run's token), and `doctor` reports the same registry the session uses.

It is a real-model check: it needs `DEEPSEEK_API_KEY` (and `KIMI_API_KEY` for kimi), uses each
model's native window (D-36), writes only under `--state-dir`, and is not part of `make check`.
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
import uuid

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

SKILL = """---
name: canary
description: A tiny probe skill that proves the registry reaches the model.
---

When you follow this skill, do exactly this:

1. Create the file `skill-report.txt` in your workspace whose content is exactly the line
   `{token}` (the token is in this body, not in the description above).
2. Then report the task as finished, mentioning the token in your summary.
"""


SEARCHABLE = """---
name: inbox-triage
description: Triages the frobnication queue when the keyword appears here and nowhere else.
---

When you follow this skill, do exactly this:

1. Create the file `triaged.txt` in your workspace whose content is exactly the line `{token}`.
2. Then report the task as finished, mentioning the token in your summary.
"""


def entries_of(state_root: pathlib.Path) -> list[str]:
    """The leader's context entries, chronologically - what the model actually saw."""
    db = sqlite3.connect(state_root / "session.sqlite")
    return [row[0] for row in db.execute(
        "SELECT message_json FROM context_entries WHERE instance_id = 'i-leader' ORDER BY idx")]



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS))
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-skills)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-skills-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    skills = root / "skills"
    token = "skill-" + uuid.uuid4().hex[:8]
    shutil.rmtree(root, ignore_errors=True)
    (skills / "canary").mkdir(parents=True)
    (skills / "inbox-triage").mkdir(parents=True)
    workspace.mkdir(parents=True)
    (skills / "canary" / "SKILL.md").write_text(SKILL.format(token=token))
    (skills / "inbox-triage" / "SKILL.md").write_text(SEARCHABLE.format(token=token))
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(
        f'skills_paths = ["{skills}"]\n\n' + MODELS[args.provider]
    )
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    # what the session's own report says about the registry (D-66)
    doctor = subprocess.run(
        [str(BIN), "doctor", "--state-root", str(state_root)], capture_output=True, text=True, env=env
    )
    registry_row = next(
        (line.strip() for line in (doctor.stdout + doctor.stderr).splitlines() if "skill(s) under" in line
         or "[WARN] skills" in line),
        "(no skills row)",
    )
    print(f"  doctor: {registry_row}")
    if token in registry_row or "2 skill(s) under 1 configured root(s)" not in registry_row:
        failures.append(f"the configured registry is not what doctor reports: {registry_row}")

    prompt = (
        "A skill named `canary` is registered for this session. Read it with the `skill` tool "
        "(action `read`, name `canary`) and follow its instructions exactly, then report the task as finished."
    )
    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), prompt],
        capture_output=True, text=True, env=env,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"provider={args.provider} exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} "
          f"goal={report.get('goal_status')}")
    if run.stderr.strip().startswith("exec:"):
        print("stderr:", run.stderr.strip()[:300])

    # 1. the skill was read on demand, and the *body* (with the run's token) is what came back
    entries = entries_of(state_root)
    calls = [entry for entry in entries if '"skill"' in entry and "canary" in entry]
    if not calls:
        failures.append("no `skill` call for canary is in the conversation: the registry never reached the model")
    # the body (not the description) is what the runtime hands back: this phrase comes
    # from the SKILL.md body, and the run's token lives in it
    receipts = [entry for entry in entries if "When you follow this skill" in entry]
    if not receipts:
        failures.append("the skill's body never reached the conversation")
    elif not any(token in entry for entry in receipts):
        failures.append("the skill receipt does not carry the body (the run's token is missing)")
    else:
        print(f"  the skill call and its body are in the conversation ({token})")

    # 2. the instructions reached the model: the file the skill asks for exists with the token
    written = workspace / "skill-report.txt"
    if not written.is_file():
        failures.append(f"the skill's instructions were not followed: {written} is missing")
    elif written.read_text().strip() != token:
        failures.append(f"the skill's instructions were not followed: {written} holds "
                        f"{written.read_text().strip()!r}, not {token!r}")
    else:
        print(f"  the model followed the skill: {written.name} carries the run's token")
    if report.get("end") not in {"reply", "completed"} or run.returncode != 0:
        failures.append(f"the run ended {report.get('end')!r} (exit {run.returncode}, "
                        f"{report.get('failure')!r})")
    if token not in json.dumps(report) and not any(token in entry for entry in entries[-4:]):
        failures.append("the model never mentioned the token it was told to report")

    search_prompt = (
        "There is a skill registered for this session that handles the frobnication queue. "
        "Find it with the `skill` tool's `search` action (query: frobnication), read what it tells you and "
        "follow it exactly, then report the task as finished."
    )
    done = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), search_prompt],
        capture_output=True, text=True, env=env,
    )
    report = json.loads(done.stdout) if done.stdout.strip().startswith("{") else {}
    print(f"  search turn: exit={done.returncode} end={report.get('end')}")
    entries = entries_of(state_root)
    def skill_actions() -> list[tuple[str, str]]:
        """Every `skill` tool call in the conversation as (action, argument)."""
        found: list[tuple[str, str]] = []
        for entry in entries_of(state_root):
            message = json.loads(entry)
            for call in message.get("tool_calls") or []:
                if call.get("function", {}).get("name") != "skill":
                    continue
                try:
                    args = json.loads(call["function"]["arguments"])
                except ValueError:
                    continue
                found.append((args.get("action", ""), args.get("name") or args.get("query", "")))
        return found

    actions = skill_actions()
    if not any(action == "search" for action, _ in actions):
        failures.append(f"the model never used the `skill` search action: {actions}")
    else:
        print(f"  the model searched the registry: {[a for a in actions]}")
    hits = [entry for entry in entries if "inbox-triage —" in entry]
    if not hits:
        failures.append("the search result did not name the skill (name - description line missing)")
    else:
        print("  the search hit names the skill and its description")
    triaged = workspace / "triaged.txt"
    if not triaged.is_file() or triaged.read_text().strip() != token:
        failures.append(f"the searched-for skill was not followed: {triaged} missing or wrong")
    else:
        print("  the model followed the skill it had to search for")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
