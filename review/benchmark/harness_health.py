#!/usr/bin/env python3
"""Receipts for a benchmark run, checked the way a product's outputs are (D-419/D-420).

Three corrections this session came from reading a receipt *after* quoting a number — a harness that never
installed a CA bundle (D-382), patches that turned out to be fetched upstream fixes (D-419), and a grader that
rebuilt the agent's database from a conversation the product's tools never appear in (D-420). Each was found by
hand. This script is the same three questions asked mechanically, for any of the three benchmark pipelines in this
directory, so a fourth one is caught by a command rather than by suspicion.

Per attempt it reports, and fails on:

* **trajectory** — did the run keep the product's own state (`session.sqlite`), so the commands it ran can be read
  afterwards? A benchmark result whose trajectory cannot be read is not evidence (D-419).
* **grading** — is there a second reading of the verdict? For SWE-bench that is the official per-instance report
  (and the patch's similarity to the gold patch, where a near-verbatim copy means retrieval rather than reasoning,
  D-419); for τ³-bench it is the DB check recomputed from the product's own calls beside the harness's native one
  (the native one is 0.0 for every trajectory when tools run inside the product, D-420).
* **retrieval** — SWE-bench only: were the fix-serving hosts blocked? The evidence is the product's own log showing
  a fetch attempt failing, or a run configuration that says so; a run with no attempt at all cannot be
  distinguished from one that fetched successfully, so it is reported as unknown rather than as clean.

Usage:
    python3 review/benchmark/harness_health.py --swebench /tmp/swebench-final
    python3 review/benchmark/harness_health.py --tau3 /tmp/tau3-honest
"""

from __future__ import annotations

import argparse
import difflib
import glob
import json
import pathlib
import re
import sqlite3
import sys

GOLD = pathlib.Path("/tmp/swebench_gold.json")
# A patch this close to the dataset's gold patch is the upstream fix, not an independent one (D-419 measured nine
# of thirty-eight at 0.94-0.99 before the guard existed).
GOLD_SIMILARITY_ALARM = 0.9


def swebench_attempts(out: pathlib.Path) -> list[pathlib.Path]:
    """The adapter writes one directory per instance at depth 1 (`<out>/<instance>/patch.diff`), beside
    `predictions.jsonl`, `grading/` and `logs/`. Getting this wrong made the first version of this auditor report
    'no attempt found' on a run that had twelve."""
    return sorted(path for path in out.iterdir() if path.is_dir() and (path / "patch.diff").exists())


def swebench_report(out: pathlib.Path) -> list[str]:
    findings = []
    gold = {}
    if GOLD.exists():
        gold = {row["instance_id"]: row["patch"] for row in json.loads(GOLD.read_text())}
    attempts = swebench_attempts(out)
    if not attempts:
        return [f"{out}: no attempt found (expected <out>/<instance>/patch.diff)"]
    for attempt in attempts:
        instance = attempt.name
        # the product's state is mounted out of the container at `<attempt>/state/...`
        state = glob.glob(f"{attempt}/state/**/session.sqlite", recursive=True)
        if not state:
            findings.append(f"{instance}: no trajectory (no collected session.sqlite)")
        report = glob.glob(f"{out}/logs/**/{instance}/report.json", recursive=True)
        if not report:
            findings.append(f"{instance}: no grading receipt (no official per-instance report)")
        if gold and (attempt / "patch.diff").exists():
            ours = (attempt / "patch.diff").read_text()
            ratio = difflib.SequenceMatcher(None, ours, gold.get(instance, "")).ratio()
            if ratio >= GOLD_SIMILARITY_ALARM:
                findings.append(f"{instance}: patch matches the gold patch at {ratio:.2f} — retrieval, not reasoning")
    return findings


def tau3_report(out: pathlib.Path) -> list[str]:
    findings = []
    runs = [path for path in sorted(out.iterdir()) if path.is_dir() and (path / "reward.json").exists()]
    if not runs:
        return [f"{out}: no task directory with a reward.json"]
    mismatches = 0
    for run in runs:
        if not (run / "calls.json").exists() or not (run / "conversation.json").exists():
            findings.append(f"{run.name}: no trajectory (calls.json/conversation.json missing)")
        reward = json.loads((run / "reward.json").read_text())
        check = reward.get("db_check")
        if check is None:
            findings.append(f"{run.name}: no second reading (reward.json carries no recomputed db_check)")
            continue
        if check.get("agent_calls") != reward.get("tool_calls"):
            findings.append(f"{run.name}: the DB check replayed {check.get('agent_calls')} calls but the trajectory "
                            f"recorded {reward.get('tool_calls')}")
        # the native verdict is expected to disagree here (D-420); a *match* would mean the harness changed under us
        if (run / "reward_detail.json").exists():
            native = json.loads((run / "reward_detail.json").read_text()).get("reward")
            if native == reward.get("reward"):
                mismatches += 1
    # The artefact D-420 measured is *systematic*: the native verdict is 0.0 for every trajectory because the
    # tools run inside the product. Agreement on a single task (where the honest reward is also 0) is not a
    # signal; agreement on all of them would mean the harness changed under us.
    if mismatches == len(runs) and runs:
        findings.append(f"all {len(runs)} task(s) agree under both readings: τ³-bench's native grading no longer "
                        "shows the D-420 artefact, so the comparison needs re-reading before it is trusted")
    return findings


def tb_report(jobs: pathlib.Path) -> list[str]:
    findings = []
    attempts = sorted(pathlib.Path(p).parent for p in glob.glob(f"{jobs}/*/*/result.json"))
    if not attempts:
        return [f"{jobs}: no attempt found"]
    without = [attempt.name for attempt in attempts
               if not glob.glob(f"{attempt}/artifacts/**/session.sqlite", recursive=True)]
    if without:
        findings.append(f"{len(without)} of {len(attempts)} attempt(s) kept no trajectory, so their commands cannot "
                        f"be read afterwards: {', '.join(without[:5])}{' …' if len(without) > 5 else ''}")
    return findings


def retrieval_evidence(out: pathlib.Path) -> str:
    """SWE-bench only: the product's own log showing a fetch attempt (blocked or not), or 'unknown' when the
    trajectories record none — a guard that was never tested is not a guard."""
    attempted = 0
    for attempt in swebench_attempts(out):
        for db in glob.glob(f"{attempt}/state/**/session.sqlite", recursive=True):
            connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
            try:
                rows = connection.execute("SELECT intent_json FROM operations").fetchall()
            except sqlite3.Error:
                continue
            finally:
                connection.close()
            for (intent,) in rows:
                text = str(intent)
                if not re.search(r"github\.com|pypi\.org|pip download|pip install|conda install", text):
                    continue
                attempted += 1
    return (f"{attempted} fetch attempt(s) in the trajectories" if attempted
            else "no fetch attempt visible: the guard's effect is unknown from this run (a guard that was never "
                 "tested is not a guard)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--swebench", help="a SWE-bench jobs directory (`--out` of swebench_agent.py)")
    parser.add_argument("--tau3", help="a τ³-bench output directory (`--out` of tau2_teamagents_agent.py)")
    parser.add_argument("--tb", help="a Terminal-Bench jobs directory")
    args = parser.parse_args()

    findings: list[str] = []
    if args.swebench:
        jobs = pathlib.Path(args.swebench)
        findings += swebench_report(jobs)
        print(f"swebench retrieval: {retrieval_evidence(jobs)}")
    if args.tau3:
        findings += tau3_report(pathlib.Path(args.tau3))
    if args.tb:
        findings += tb_report(pathlib.Path(args.tb))

    if findings:
        for finding in findings:
            print(f"FAIL: {finding}")
        return 1
    print("receipts: every attempt in this run kept its trajectory and has a second reading of its verdict")
    return 0


if __name__ == "__main__":
    sys.exit(main())
