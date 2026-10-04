#!/usr/bin/env python3
"""D-415: does `[limits] max_tool_seconds` convert long single tool calls into shorter ones without lowering the
pass rate? (Pre-registered in D-414: the claim, its measurement and the inverse reading that would kill it.)

Arms differ only in `TEAMAGENTS_MAX_TOOL_SECONDS` (unset = the product's own behaviour). Tasks are the
tool-dominated family D-414 measured, and every attempt collects its state so `review/benchmark/cadence.py` can
show whether the ceiling actually cut the calls (a score-only comparison would not distinguish "the cap worked"
from "the model stopped asking").

Measured 2026-10-04 (`-n 4 -k 3`, `low`, official endpoint, 890 s client bound, each task's own budget):
`bare` **4/12** against `ceiling` **5/12** — `code-from-image` 2/3 → 3/3, `query-optimize` 0/3 → 1/3,
`gpt2-codegolf` 2/3 → 1/3, `winning-avg-corewars` 0/3 → 0/3 — with the mechanism confirmed by `cadence.py`:
the largest single tool call **181 s → 60 s**, total tool time 3481 s → 2803 s (−19 %), operations 801 → 700,
wall clock 7430 s → 6846 s, and the settled/cut split unchanged at 6/6. At twelve trials per arm the pass-rate
difference is far inside the noise (D-388's band), so the honest reading is *the mechanism works and the rate is
not measurably affected*, not "the ceiling raises the score".
"""
import glob, json, os, pathlib, shutil, subprocess, sys
from collections import Counter

ENV = {**os.environ, "PATH": "/tmp/harbor-venv/bin:" + os.environ["PATH"], "DOCKER_BUILDKIT": "1",
       "TEAMAGENTS_REASONING_EFFORT": "low", "TEAMAGENTS_TURN_TIMEOUT_SEC": "890",
       "PYTHONPATH": "/home/rimuru/Projects/Code/for_fun/TeamAgents/review/benchmark"}
TREE = "/tmp/ta-harbor/ceiling4"


def run(arm: str, ceiling: str) -> str:
    out = f"/tmp/ta-harbor/ceiling4-jobs-{arm}"
    shutil.rmtree(out, ignore_errors=True)
    env = dict(ENV)
    if ceiling:
        env["TEAMAGENTS_MAX_TOOL_SECONDS"] = ceiling
    done = subprocess.run(["harbor", "run", "-p", TREE, "--agent", "teamagents_agent:TeamAgentsAgent",
                           "-m", "deepseek-flash", "-y", "-n", "4", "-k", "3", "-o", out],
                          env=env, capture_output=True, text=True)
    if done.returncode != 0:
        print(f"!! {arm}: rc={done.returncode}: {done.stderr[-300:]}", file=sys.stderr, flush=True)
    ends, passed = Counter(), 0
    for p in glob.glob(f"{out}/*/*/result.json"):
        r = json.load(open(p))
        if ((r.get("verifier_result") or {}).get("rewards") or {}).get("reward") == 1.0:
            passed += 1
        exc = (r.get("exception_info") or {}).get("exception_type")
        ends["cut" if exc == "AgentTimeoutError" else (exc or "settled")] += 1
    print(f"{arm:10} passed={passed}/{sum(ends.values())} ends={dict(ends)} -> {out}", flush=True)
    return out


if __name__ == "__main__":
    run("bare", "")
    run("ceiling", "60")
