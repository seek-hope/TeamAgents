#!/usr/bin/env python3
"""Freeze the R24 manifest: task digests, model/profile parameters, analysis rule."""
import hashlib, json, pathlib, subprocess

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent


def digest_tree(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    for file in sorted(p for p in path.rglob("*") if p.is_file()):
        h.update(str(file.relative_to(path)).encode())
        h.update(file.read_bytes())
    return h.hexdigest()


import sys

only = sys.argv[2].split(",") if len(sys.argv) > 2 and sys.argv[2] else None
tasks = []
for task_dir in sorted(p for p in (HERE / "tasks").iterdir() if p.is_dir()):
    if only and task_dir.name not in only:
        continue
    tasks.append({
        "id": task_dir.name,
        "prompt_sha256": hashlib.sha256((task_dir / "prompt.md").read_bytes()).hexdigest(),
        "checks": (task_dir / "checks.txt").read_text(encoding="utf-8").splitlines(),
        "fixture_sha256": digest_tree(task_dir / "fixture") if (task_dir / "fixture").is_dir() else None,
    })

manifest = {
    "frozen": "2026-09-24",
    "design": "review/eval/r2-p6/design.md",
    "groups": {
        "A": "reference loop (no persistence, eval-only)",
        "B": "v2 persistent single instance",
        "C": "v2 supervisor + collaboration grants/instructions",
    },
    "model": {
        "key": "leader_main",
        "wire_model": "deepseek-flash",
        "context_window": 1000000,
        "context_source": "D-36: user-confirmed DeepSeek Flash native 1M (2026-09-17)",
        "reasoning_effort": "high",
        "effort_note": "catalog default max (~240s per simple turn); the pilot phase sets high explicitly to bound cost and duration",
    },
    "limits": {"trial_timeout_s": 900, "reference_max_steps": 40, "permissions": "full_auto", "max_retries": 2},
    "repeats": {"pilot": 1, "formal": 3},
    "analysis": {
        "metric": "checks_ok (all checks exit 0 in the trial workspace)",
        "pairing": "same repeat index within a task",
        "aggregate": "mean of per-task paired differences",
        "interval": "95% bootstrap over tasks, 10000 resamples, seed 20260924",
        "h1": "B vs A: no observed degradation; interval must not be significantly negative",
        "h2": "C vs B: reproducible gain; per-task positive on >=2 tasks and interval lower bound > 0",
        "insufficient": "interval contains 0 or too few samples -> report not confirmed",
    },
    "tasks": tasks,
}
analysis = HERE / "analyze.py"
manifest["analysis"]["sha256"] = hashlib.sha256(analysis.read_bytes()).hexdigest() if analysis.exists() else None
manifest["git"] = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip()
(HERE / (sys.argv[1] if len(sys.argv) > 1 else "manifest.json")).write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
print("manifest frozen:", len(tasks), "tasks ->", sys.argv[1] if len(sys.argv) > 1 else "manifest.json")
