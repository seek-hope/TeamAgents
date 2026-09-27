#!/usr/bin/env python3
"""A real-model check that a verified input which changed blocks the goal (A17).

`checks.py` (A16) shows a check that can never pass. A17 is the subtler half of the same gate: the check
*passes*, and then it turns out the thing it verified is not the thing that would be delivered. The offline
test `v2_driver::check_inputs_must_still_hold_at_completion` drives that with a check that mutates its own
declared input; this probe does it with a real model:

1. a `[[checks]]` entry declares `inputs = ["out.txt"]` and its command writes `changed` into that very file
   (so the check exits 0 while invalidating what it observed);
2. the model is asked to write `out.txt` with the content `original` and to finish;
3. the runtime must notice that the declared input no longer matches what the check round observed: it records
   the verdict (a `completion_repair` carrying `class: "stale_inputs"`) and states it in the conversation for
   the repair turn, and the goal ends **BLOCKED** — either parked by the runtime, whose reason then names
   `bound:stale_inputs`, or settled by the model's own blocked report. No run reports success either way; which
   of the two endings happens is the model's choice, so the assertion is on what must hold in both (D-187).

The artifact decides both halves: the file really exists and holds one of the two values the scenario writes,
and the session never claims the goal was done. Which of the two it holds is not an invariant — the model's
repair turn rewrites its deliverable, so the last writer is the model's when it settles the goal itself and the
check's when the runtime parks it (the assertion here said "the check's content" until a round-2 run ended with
the model's, D-204); that the check ran at all is the runtime's own `stale_inputs` record.

    python3 review/dogfood/stale_check.py                  # DeepSeek
    python3 review/dogfood/stale_check.py --provider kimi  # over `responses`
    python3 review/dogfood/stale_check.py --state-dir /tmp/ta-stale --timeout 420

It is a real-model check: it needs `DEEPSEEK_API_KEY` (or `KIMI_API_KEY` with `--provider kimi`), uses each
model's native window (D-36), and writes only under `--state-dir`.
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
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
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

# the check passes (printf exits 0) and invalidates the input it declared
CHECK = """[[checks]]
id = "bound"
command = "printf changed > out.txt"
inputs = ["out.txt"]
timeout = 60
"""

PROMPT = """Create the file out.txt in this workspace whose content is exactly the line `original`.
Then report the task as finished."""


def config_text(provider: str) -> str:
    return (
        "# Stale-input dogfood (A17): the check below rewrites the very file it declares as its\n"
        "# input, so the value it verified cannot be the value that would be delivered.\n"
        "skills_paths = []\n\n" + MODELS[provider] + "\n" + CHECK
    )


def session_facts(state_root: pathlib.Path) -> dict:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    goals = list(db.execute("SELECT id, status, limits_json FROM goals ORDER BY rowid"))
    events = [(row[0], json.loads(row[1])) for row in db.execute("SELECT kind, payload_json FROM events ORDER BY sequence")]
    entries = [row[0] for row in db.execute("SELECT message_json FROM context_entries ORDER BY idx")]
    requests = list(db.execute("SELECT COUNT(*) FROM model_requests"))[0][0]
    return {"goals": goals, "events": events, "entries": entries, "requests": requests}



def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started.

    `exec` autostarts one and it is detached on purpose (background work survives a client exit, §9), so
    without this a probe would leave a live session behind on the user's machine. Registered with `atexit`,
    which also covers the early returns above.
    """
    leak_guard.stop_daemons(state_root)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-stale)")
    parser.add_argument("--timeout", type=int, default=420, help="exec --timeout in seconds")
    parser.add_argument("--provider", default="deepseek", choices=sorted(MODELS),
                        help="which catalog entry the session runs on (the check round's wire differs per protocol)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    key_env = "KIMI_API_KEY" if args.provider == "kimi" else "DEEPSEEK_API_KEY"
    if not os.environ.get(key_env, "").strip():
        raise SystemExit(f"{key_env} is not set in this environment")

    root = pathlib.Path(args.state_dir or f"/tmp/ta-stale-{args.provider}")
    # The default scratch is not state anyone keeps: remove it at exit, or one copy per run
    # accumulates in TMPDIR (D-138, the defect D-131 fixed for the test suite). An explicit
    # --state-dir is left alone, because the caller asked for it.
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(config_text(args.provider))
    state_root = root / "root"
    atexit.register(stop_daemon, root)
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []

    started = time.time()
    run = subprocess.run(
        [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
         "--timeout", str(args.timeout), "--cwd", str(workspace), PROMPT],
        capture_output=True, text=True, env=env,
    )
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    print(f"provider={args.provider} exec exit={run.returncode} elapsed={elapsed}s end={report.get('end')} "
          f"goal={report.get('goal_status')}")
    if run.stderr.strip():
        print("stderr:", run.stderr.strip()[:300])

    facts = session_facts(state_root)
    kinds = [kind for kind, _ in facts["events"]]
    print(f"model requests: {facts['requests']} | goals: {facts['goals']}")
    for kind in ("check_round_registered", "completion_repair", "goal_completed"):
        print(f"  {kind}: {kinds.count(kind)}")

    # 1. the work happened, and the check really ran. Two writers touch out.txt — the model's deliverable and
    #    the check's own `printf changed` — and which one lands *last* depends on the ending the model chose:
    #    settling the goal itself means it re-wrote the file on its repair turn (the round-2 run of 2026-09-27
    #    ends with the model's content), while a runtime park leaves the check's. Both are honest, so the
    #    assertion is that the file exists holding one of the two values this scenario writes, not which one;
    #    that the check ran is the runtime's own record, read in §3 below (D-197: an assertion may not require
    #    an order the claim does not).
    out = workspace / "out.txt"
    held = out.read_text().strip() if out.is_file() else ""
    if held in ("changed", "original"):
        print(f"out.txt holds {held!r}, one of the two values this scenario writes")
    else:
        failures.append(f"out.txt is missing or holds {held or '(absent)'!r}, neither the model's deliverable "
                        "nor the check's own content")
    # ...and the model really wrote it first (the write receipt is in the conversation)
    wrote = [entry for entry in facts["entries"] if "out.txt" in entry and '"tool"' in entry.replace(" ", "")]
    if not wrote:
        failures.append("the model never wrote out.txt, so the gate was never reached")
    else:
        print("the conversation carries the model's write of out.txt")

    # 2. no success was reported: the stale input gated the goal
    if run.returncode == 0:
        failures.append("the headless run exited 0 although its verified input had changed")
    if report.get("end") != "failed":
        failures.append(f"a run whose verified input changed must end failed, not {report.get('end')!r}")
    statuses = {goal[1] for goal in facts["goals"]}
    if "SUCCEEDED" in statuses:
        failures.append(f"a goal was reported SUCCEEDED although its verified input changed: {facts['goals']}")
    if "BLOCKED" not in statuses:
        failures.append(f"the goal did not end BLOCKED: {facts['goals']}")

    # 3. the runtime recorded the verdict, and the model was told about it
    repairs = [payload for kind, payload in facts["events"] if kind == "completion_repair"]
    verdicts = [failure for payload in repairs for failure in payload.get("failures", [])
                if failure.get("check_id") == "bound" and failure.get("class") == "stale_inputs"]
    if not verdicts:
        failures.append(f"no completion_repair names bound/stale_inputs: {repairs}")
    else:
        print(f"the runtime recorded the stale input: {verdicts[0].get('reason')!r}")
    # §8/D-187: the verdict is the *runtime's* own — the check itself exited 0 — so the repair turn has to state
    # it; before that entry the model saw only "the required checks must pass" and had to guess (a live run
    # re-derived the workspace state over eleven requests before settling the goal itself).
    seen = [entry for entry in facts["entries"] if "stale_inputs" in entry]
    if not seen:
        failures.append("the model was never told why its completion was refused: no stale_inputs in the conversation")
    else:
        print("the conversation carries the verdict")

    # 4. the settlement: either the runtime parked the goal — its reason names the stale input — or the model
    #    reported the divergence itself. Both are honest endings; a success claim is not, and the model chooses
    #    which one happens (the recorded run of 2026-09-26 parked after three rounds; the run of 2026-09-27
    #    settled itself at round two with a blocked report), so the assertion is on what must hold, not on which
    #    path the model took.
    settled = [payload for kind, payload in facts["events"] if kind == "goal_completed"]
    if not settled or settled[-1].get("status") != "BLOCKED":
        failures.append(f"the goal did not settle BLOCKED: {settled}")
    elif "reason" in settled[-1]:
        if "bound:stale_inputs" not in settled[-1]["reason"]:
            failures.append(f"the runtime parked the goal without naming the stale input: {settled[-1]['reason']!r}")
        else:
            print(f"the runtime parked the goal naming the stale input: {settled[-1]['reason']}")
    else:
        outcome = (settled[-1].get("completion") or {}).get("outcome")
        if outcome != "blocked":
            failures.append(f"the model settled the goal as {outcome!r} although its verified input had changed")
        else:
            print("the model settled the goal itself with a blocked report (no runtime park to name the input)")

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
