#!/usr/bin/env python3
"""Run TeamAgents on SWE-bench Verified instances and grade with the **official** harness (D-419).

Why a second benchmark at all: every measurement before this one came from Terminal-Bench 2.1, and a product
tuned against one benchmark's tasks is the overfitting trap. SWE-bench Verified is the other authoritative
coding-agent benchmark — real repositories, real issues, and a grader (the repository's own tests) that no
harness can talk to — so a result here is evidence about the product, not about one dataset's shape.

The pipeline, per instance:

1. the **official** image (``swebench/sweb.eval.x86_64.<id>``) is pulled, so the environment is the one the
   grader will use, not one this adapter built;
2. the product runs inside it: the static ``teamagents`` binary mounted at ``/usr/local/bin``, the frozen config
   mounted read-only, ``DEEPSEEK_API_KEY`` from the environment, ``--cwd /testbed`` and a state root **outside**
   the repository (so the product's own files cannot land in the patch);
3. the patch is ``git add -A && git diff HEAD`` inside ``/testbed`` — the repository's tracked changes, which is
   exactly what the harness applies to a fresh container;
4. grading is ``python3 -m swebench.harness.run_evaluation`` on the predictions file, so the verdict is the
   official one (FAIL_TO_PASS / PASS_TO_PASS), not this script's opinion.

Usage:
    python3 review/benchmark/swebench_agent.py --instances ID [ID ...] --out DIR [--timeout 1200] [--jobs 2]
    python3 review/benchmark/swebench_agent.py --instances ID --out DIR --grade-only

The prompt is fixed and stated here so a run can be reproduced: the task's own ``problem_statement``, prefixed
by one instruction line (below). No harness-specific scaffolding, no retries, no test patch (the grader applies
it), and nothing about the repository's test names reaches the model.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

REPO = pathlib.Path(__file__).resolve().parents[2]
BINARY = pathlib.Path(os.environ.get("TEAMAGENTS_BINARY", REPO / "engine/target/x86_64-unknown-linux-musl/release/teamagents"))
MODEL_NAME = os.environ.get("TEAMAGENTS_SWEBENCH_MODEL", "teamagents-deepseek-flash")
DATASET = "SWE-bench/SWE-bench_Verified"

# The one instruction line, fixed: the model gets the issue and the checkout, nothing else.
INSTRUCTION = (
    "Fix the following issue in this repository (the checkout is /testbed). Change the repository's own code; "
    "do not add or modify tests. When the fix is in place, report it with the finish tool.\n\n"
)

PINNED_CONFIG = """\
[models.leader_main]
provider = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
"""


def instance_row(instance_id: str) -> dict:
    """The instance's metadata, from the fetched dataset copy (`review/benchmark/swebench_verified_rows.json`)."""
    path = REPO / "review/tmp/swebench/verified_rows.json"
    if not path.exists():
        raise SystemExit(
            f"{path} is missing: fetch the Verified rows into the ignored scratch (the docstring's --fetch note):\n"
            "  curl -sS 'https://datasets-server.huggingface.co/rows?dataset=SWE-bench%2FSWE-bench_Verified"
            "&config=default&split=test&offset=0&length=50' … (one page per 50 instances, concatenated)"
        )
    for row in json.loads(path.read_text()):
        if row["instance_id"] == instance_id:
            return row
    raise SystemExit(f"{instance_id} is not in the fetched Verified rows")


def image_for(instance_id: str) -> str:
    return f"swebench/sweb.eval.x86_64.{instance_id.replace('__', '_1776_')}:latest"


def run_one(instance: dict, out: pathlib.Path, timeout: int) -> dict:
    instance_id = instance["instance_id"]
    image = image_for(instance_id)
    print(f"== {instance_id}: {image}", flush=True)
    subprocess.run(["docker", "pull", image], check=True, capture_output=True, text=True, timeout=3600)
    work = out / instance_id
    work.mkdir(parents=True, exist_ok=True)
    (work / "config.toml").write_text(PINNED_CONFIG)
    (work / "state").mkdir(exist_ok=True)
    prompt = INSTRUCTION + instance["problem_statement"]
    (work / "prompt.txt").write_text(prompt)
    # The image can already be dirty (`build/` in the requests image, for instance). Those paths are recorded
    # before the agent runs and unstaged afterwards, so the patch carries the agent's work and nothing the image
    # shipped with — a patch the grader applies must not contain the environment's own leftovers.
    command = (
        "mkdir -p /opt/ta/state /opt/ta/config/teamagents && "
        "cp /opt/ta-in/config.toml /opt/ta/config/teamagents/config.toml && "
        "cd /testbed && git status --porcelain | awk '{print $2}' > /opt/ta-in/baseline.txt; "
        "XDG_CONFIG_HOME=/opt/ta/config XDG_STATE_HOME=/opt/ta/state "
        f"timeout {timeout} /usr/local/bin/teamagents exec --json --full-auto --timeout {timeout} "
        "--state-root /opt/ta/state/teamagents/v2 --cwd /testbed -- \"$(cat /opt/ta-in/prompt.txt)\" "
        "> /opt/ta-in/report.json 2>&1; echo \"exec rc=$?\" >> /opt/ta-in/report.json; "
        "cd /testbed && git add -A && while read -r path; do [ -n \"$path\" ] && git reset -q -- \"$path\" 2>/dev/null; "
        "done < /opt/ta-in/baseline.txt; git diff HEAD > /opt/ta-in/patch.diff"
    )
    started = subprocess.run(
        [
            "docker", "run", "--rm",
            "-v", f"{BINARY}:/usr/local/bin/teamagents:ro",
            "-v", f"{work}:/opt/ta-in",
            "-e", "DEEPSEEK_API_KEY",
            "-w", "/testbed",
            image, "bash", "-lc", command,
        ],
        capture_output=True, text=True, timeout=timeout + 600,
    )
    patch = (work / "patch.diff").read_text() if (work / "patch.diff").exists() else ""
    report = (work / "report.json").read_text() if (work / "report.json").exists() else ""
    (work / "docker.log").write_text(started.stdout + started.stderr)
    print(f"   patch: {len(patch.splitlines())} line(s); docker rc={started.returncode}", flush=True)
    return {"instance_id": instance_id, "model_name_or_path": MODEL_NAME, "model_patch": patch, "report": report}


def grade(predictions: pathlib.Path, instances: list[str], out: pathlib.Path) -> int:
    command = [
        sys.executable, "-m", "swebench.harness.run_evaluation",
        "-d", DATASET,
        "-i", *instances,
        "-p", str(predictions),
        "-id", "teamagents",
        "--max_workers", "2",
        "-t", "1800",
        "--report_dir", str(out / "grading"),
    ]
    print("+ " + " ".join(command), flush=True)
    # The harness writes `logs/run_evaluation/<run>/...` relative to the *cwd*; running it from the run's own
    # output directory keeps those logs with the evidence they belong to instead of in the repository root.
    return subprocess.call(command, cwd=str(out))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--instances", nargs="+", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--timeout", type=int, default=1200, help="per-instance turn bound in seconds")
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--grade-only", action="store_true")
    args = parser.parse_args()

    out = pathlib.Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    predictions = out / "predictions.jsonl"
    if not args.grade_only:
        rows = [instance_row(instance) for instance in args.instances]
        with ThreadPoolExecutor(max_workers=args.jobs) as pool:
            results = list(pool.map(lambda row: run_one(row, out, args.timeout), rows))
        with predictions.open("w") as handle:
            for result in results:
                handle.write(json.dumps({"instance_id": result["instance_id"],
                                         "model_name_or_path": result["model_name_or_path"],
                                         "model_patch": result["model_patch"]}) + "\n")
        for result in results:
            print(f"{result['instance_id']}: {len(result['model_patch'].splitlines())} patch line(s)")
    if not predictions.exists():
        raise SystemExit(f"{predictions} does not exist: nothing to grade")
    return grade(predictions, args.instances, out)


if __name__ == "__main__":
    sys.exit(main())
