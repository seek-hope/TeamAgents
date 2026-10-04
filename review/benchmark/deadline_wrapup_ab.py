#!/usr/bin/env python3
"""D-410: does telling a goal its deadline is close convert *cuts* into *settlements*?

The arms differ in one harness variable, `TEAMAGENTS_DEADLINE_MINUTES` (the config's `[limits] deadline_minutes`):

* `bare`   — no goal deadline, so the client's own `TEAMAGENTS_TURN_TIMEOUT_SEC` cuts the turn.
* `deadline` — a deadline inside that window, so the runtime warns the model when ~120 s remain and refuses the
  requests that follow the deadline. The task's own budget still governs the whole attempt.

The measured set is the tasks that actually hit the deadline in the N=3 full-set run (D-389), one attempt per arm.

**Two ways this measurement lies, both measured and both kept in the record (D-410).**

1. A deadline **after** the client's cut (`--minutes 15` against `--turn-timeout 890`) never arrives inside the
   turn: an instrumented check of three tasks found `goal_deadline_warning` events = 0 and no note in any
   conversation, so the arm's higher pass count cannot be credited to the warning — it is the deadline ending runs
   a few seconds early plus run-to-run variation. This script therefore *always* reports, per arm, whether the
   warning fired (from `deadline_warnings.json`, written by the adapter's artifact collection), so a void
   measurement is visible as a measurement rather than as an effect.
2. A warning is delivered at a **step boundary**. A run inside one long tool call when the window closes receives
   neither the warning nor, mid-call, the deadline. That is why the effect at `--minutes 13` was small: 2 of 12
   runs were between steps when the note was due.

Run it with credentials in the environment (`DEEPSEEK_API_KEY`), a local `harbor`, the built musl binary, and the
task tree this script expects (see `review/benchmark/README.md`). It writes one jobs directory per arm under
`--jobs`, so a re-run of the analysis (`--analyze-only`) needs no containers.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import pathlib
import shutil
import subprocess
import sys
from collections import Counter

# The 12 tasks that hit their deadline in D-389's N=3 run: the population the wrap-up note is for.
TASKS = [
    "adaptive-rejection-sampler", "code-from-image", "compile-compcert", "extract-moves-from-video",
    "filter-js-from-html", "gcode-to-text", "gpt2-codegolf", "make-doom-for-mips", "protein-assembly",
    "torch-pipeline-parallelism", "train-fasttext", "winning-avg-corewars",
]
ADAPTER_PATH = "/home/rimuru/Projects/Code/for_fun/TeamAgents/review/benchmark"


def run_arm(arm: str, tree: str, jobs: str, minutes: str, args: argparse.Namespace) -> None:
    out = f"{jobs}-{arm}"
    shutil.rmtree(out, ignore_errors=True)
    env = {
        **os.environ,
        "PATH": "/tmp/harbor-venv/bin:" + os.environ["PATH"],
        "DOCKER_BUILDKIT": "1",
        "TEAMAGENTS_TURN_TIMEOUT_SEC": str(args.turn_timeout),
        "TEAMAGENTS_REASONING_EFFORT": args.effort,
        "PYTHONPATH": ADAPTER_PATH,
    }
    if minutes:
        env["TEAMAGENTS_DEADLINE_MINUTES"] = minutes
    done = subprocess.run(
        ["harbor", "run", "-p", tree, "--agent", "teamagents_agent:TeamAgentsAgent",
         "-m", args.model, "-y", "-n", str(args.concurrency), "-k", "1", "-o", out],
        env=env, capture_output=True, text=True,
    )
    if done.returncode != 0:
        print(f"!! {arm}: harbor rc={done.returncode}: {done.stderr[-300:]}", file=sys.stderr, flush=True)
    return out


def mechanism(attempt: pathlib.Path) -> tuple[int, int]:
    """The mechanism's own receipt, read from the state the run committed (never inferred from the outcome):
    the `goal_deadline_warning` events and the `goal-deadline-warning-<goal>` entries in the conversation."""
    import sqlite3
    warned = notes = 0
    for db in attempt.glob("artifacts/**/session.sqlite"):
        connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        warned += connection.execute(
            "SELECT COUNT(*) FROM events WHERE kind = 'goal_deadline_warning'").fetchone()[0]
        notes += connection.execute(
            "SELECT COUNT(*) FROM context_entries WHERE envelope_id LIKE 'goal-deadline-warning-%'").fetchone()[0]
        connection.close()
    return warned, notes


def analyze(arm: str, out: str) -> dict:
    ends, passed, warned, notes, receipts = Counter(), 0, 0, 0, 0
    per_task: dict[str, float] = {}
    for result in sorted(glob.glob(f"{out}/*/*/result.json")):
        attempt = pathlib.Path(result).parent
        report = json.load(open(result))
        # harbor names an attempt `<task>__<hash>`, and the run directory above it is `<date>__<time>`: the last
        # component carrying `__` is the task, not the date
        task = next((part.split("__")[0] for part in reversed(attempt.relative_to(out).parts) if "__" in part),
                    attempt.name)
        reward = ((report.get("verifier_result") or {}).get("rewards") or {}).get("reward")
        per_task[task] = reward
        passed += reward == 1.0
        exception = (report.get("exception_info") or {}).get("exception_type")
        ends["cut" if exception == "AgentTimeoutError" else (exception or "settled")] += 1
        fired, recorded = mechanism(attempt)
        warned += fired
        notes += recorded
        receipts += bool(list(attempt.glob("artifacts/**/session.sqlite")))
    # a missing receipt is not a missing warning: with no state collected the mechanism cannot be seen at all
    print(f"{arm:10} passed={passed}/{sum(ends.values())} ends={dict(ends)} "
          f"warnings={warned} notes_in_conversation={notes} state_receipts={receipts}/{sum(ends.values())}",
          flush=True)
    for task, reward in sorted(per_task.items()):
        print(f"    {task:32} {reward}")
    return {"passed": passed, "ends": dict(ends), "warnings": warned, "notes": notes,
            "receipts": receipts, "attempts": sum(ends.values())}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--tree", required=True, help="the task tree (the 12 tasks, with artifact collection)")
    parser.add_argument("--jobs", required=True, help="jobs directory prefix (one directory per arm)")
    parser.add_argument("--minutes", default="13", help="`TEAMAGENTS_DEADLINE_MINUTES` for the deadline arm")
    parser.add_argument("--turn-timeout", default=890, type=int, help="the client's own bound, in seconds")
    parser.add_argument("--model", default="deepseek-flash")
    parser.add_argument("--effort", default="low", help="the reasoning effort both arms use")
    parser.add_argument("--concurrency", default=6, type=int)
    parser.add_argument("--analyze-only", action="store_true", help="read the two jobs directories, start nothing")
    args = parser.parse_args()

    if not args.analyze_only:
        run_arm("bare", args.tree, args.jobs, "", args)
        run_arm("deadline", args.tree, args.jobs, args.minutes, args)
    bare = analyze("bare", f"{args.jobs}-bare")
    deadline = analyze("deadline", f"{args.jobs}-deadline")
    if deadline["receipts"] < deadline["attempts"]:
        print(f"\nINCOMPLETE receipt: only {deadline['receipts']} of {deadline['attempts']} deadline-arm attempts "
              "collected their state, so `warnings` cannot show whether the note fired — add "
              '`artifacts = ["/opt/teamagents/state"]` to each task.toml and re-run.', file=sys.stderr)
    elif deadline["warnings"] == 0:
        print("\nVOID as evidence about the warning: no `goal_deadline_warning` reached any conversation in the\n"
              "deadline arm. Try a `--minutes` that lands inside the turn (shorter than `--turn-timeout`), and keep\n"
              "this run as the record of a deadline-only effect.", file=sys.stderr)
    print(f"\nbare {bare['passed']} vs deadline {deadline['passed']}; "
          f"warnings fired {deadline['warnings']} (the effect is only attributable to the warning above 0)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
