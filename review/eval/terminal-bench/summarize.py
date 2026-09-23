#!/usr/bin/env python3
"""Summarize a Harbor job directory for the TeamAgents Terminal-Bench runs.

Prints the headline numbers, a per-task table, and the list of trials whose
failure was infrastructure (image pull / agent install) instead of a real
measurement — those are the ones worth re-running.

Usage: python3 review/eval/terminal-bench/summarize.py <job-dir> [--markdown]
"""

from __future__ import annotations

import collections
import glob
import json
import os
import sys


def classify(exception: dict | None) -> str:
    if not exception:
        return "ok"
    message = (exception.get("exception_message") or "").lower()
    if "docker compose command failed" in message or "failed to resolve reference" in message:
        return "infra:image"
    if "bubblewrap" in message or "no such file or directory" in message:
        return "infra:agent-install"
    if "timed out" in message:
        return "agent-timeout"
    return "error:" + str(exception.get("exception_type"))


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    job = sys.argv[1]
    markdown = "--markdown" in sys.argv
    rows: list[tuple[str, float | None, str, int, int | None]] = []
    for trial in sorted(glob.glob(os.path.join(job, "*__*"))):
        name = os.path.basename(trial).split("__")[0]
        result_path = os.path.join(trial, "result.json")
        if not os.path.exists(result_path):
            rows.append((name, None, "running", 0, None))
            continue
        with open(result_path, encoding="utf-8") as handle:
            result = json.load(handle)
        reward = ((result.get("verifier_result") or {}).get("rewards") or {}).get("reward")
        kind = classify(result.get("exception_info"))
        usage = result.get("agent_result") or {}
        lines = 0
        log = os.path.join(trial, "agent", "teamagents.jsonl")
        if os.path.exists(log):
            lines = sum(1 for _ in open(log))
        rows.append(
            (
                name,
                reward,
                kind,
                lines,
                (usage.get("n_input_tokens"), usage.get("n_output_tokens")) if usage else None,
            )
        )

    scored = [(name, reward, tokens) for name, reward, kind, _, tokens in rows if reward is not None]
    infra = [row for row in rows if row[2].startswith("infra")]
    timeouts = [row for row in rows if row[2] == "agent-timeout"]
    others = [row for row in rows if row[2] not in ("ok",) and not row[2].startswith("infra") and row[2] != "agent-timeout" and row[2] != "running"]
    passed = [row for row in scored if row[1] == 1.0]
    tokens_in = sum((tokens or (0, 0))[0] or 0 for _, _, tokens in scored)
    tokens_out = sum((tokens or (0, 0))[1] or 0 for _, _, tokens in scored)

    print(f"job: {job}")
    print(f"trials: {len(rows)}  finished: {len(rows) - len([r for r in rows if r[2] == 'running'])}")
    print(f"scored: {len(scored)}  passed: {len(passed)}  mean reward: {sum(r[1] for r in scored)/len(scored):.4f}" if scored else "scored: 0")
    print(f"infra failures: {len(infra)}  agent timeouts: {len(timeouts)}  other errors: {len(others)}")
    if scored:
        print(f"tokens over scored trials: in={tokens_in:,} out={tokens_out:,}")
        print(f"infrastructure-adjusted mean (scored only): {sum(r[1] for r in scored)/len(scored):.4f}")
    print()
    header = "| task | reward | outcome | jsonl | in_tokens | out_tokens |"
    print(header if markdown else header.replace("|", " ").replace("---", ""))
    if markdown:
        print("| --- | --- | --- | --- | --- | --- |")
    for name, reward, kind, lines, tokens in rows:
        in_tok, out_tok = tokens if tokens else (None, None)
        reward_text = "-" if reward is None else f"{reward:.2f}"
        print(f"| {name} | {reward_text} | {kind} | {lines} | {in_tok or ''} | {out_tok or ''} |" if markdown
              else f"{name:40s} {reward_text:>6s} {kind:22s} jsonl={lines:5d} in={in_tok} out={out_tok}")
    if infra:
        print("\nrerun (infrastructure):")
        for name, _, kind, _, _ in infra:
            print(f"  {name} ({kind})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
