#!/usr/bin/env python3
"""Freeze an R2-P6 manifest: task digests, model/profile parameters, the surface pin and the analysis rule.

    python3 review/eval/r2-p6/freeze.py manifest.json                      # the recorded rounds' default
    python3 review/eval/r2-p6/freeze.py manifest-r5.json six-deliverables,six-mixed,multi-step \\
        --design review/eval/r2-p6/design-r5.md --groups '{"B": "...", "D": "..."}' \\
        --limits '{"trial_timeout_s": 900, "reference_max_steps": 25, "permissions": "full_auto", "max_retries": 2}' \\
        --task-timeout six-deliverables=55 --task-timeout six-mixed=48 --note "..." --config eval-config.toml

Everything the audits read is *derived*: each task's digests come from the tree, the surface pin is recomputed
from the harness by `review/eval_surface.py`, and the analysis rule's digest is `analyze.py`'s. The optional
flags exist because round 5 varies what the recorded rounds fixed (a two-group treatment, a per-task wall-clock
bound, its own design document); omitting them reproduces the earlier manifests' shape.
"""
import argparse
import hashlib
import json
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent

sys.path.insert(0, str(HERE.parents[1]))  # review/, where eval_surface.py lives
import eval_surface  # the decoder behind D-182: the surface pin is computed from the harness, never copied


def is_cache(path: pathlib.Path) -> bool:
    """True for a file or path component that is a build artefact rather than fixture material."""
    return path.suffix == ".pyc" or "__pycache__" in path.parts


def digest_tree(path: pathlib.Path) -> str:
    """A fixture's digest, over its *source* files.

    D-259: pytest's byte-cache is not material. A stray `__pycache__` inside a fixture (created by anything that
    runs a unit test where the fixture lives) moved a frozen digest once, and the digest a manifest pins must not
    depend on whether somebody ran pytest there."""
    h = hashlib.sha256()
    for file in sorted(p for p in path.rglob("*") if p.is_file() and not is_cache(p)):
        h.update(str(file.relative_to(path)).encode())
        h.update(file.read_bytes())
    return h.hexdigest()


def surface_pin() -> dict:
    """The harness's half of the model-visible surface (D-182), read out of the code that carries it.

    `review/eval_surface.py` recomputes every field here from the tree and from the recorded trials, so a
    hand-edited pin cannot pass. The product's half (prompt assembly, the tools a grant adds) is not pinned
    here: each batch records the commit that holds it.
    """
    source = (REPO / eval_surface.HARNESS).read_text(encoding="utf-8")
    agent, team, directive = eval_surface.surface_of(source)
    pin = {
        "pinned": "2026-09-27",
        "note": "the harness's half of the model-visible surface; recomputed from the tree by review/eval_surface.py",
        "agent_instructions": agent,
        "agent_instructions_sha256": eval_surface.digest(agent),
        "team_instructions": team,
        "team_instructions_sha256": eval_surface.digest(team),
        "team_directive_instructions": directive,
        "team_directive_instructions_sha256": eval_surface.digest(directive),
        "tools": eval_surface.tools_of((REPO / eval_surface.REFERENCE).read_text(encoding="utf-8")),
        "reasoning_effort": "high",
    }
    return pin


DEFAULT_GROUPS = {
    "A": "reference loop (no persistence, eval-only)",
    "B": "v2 persistent single instance",
    "C": "v2 supervisor + collaboration grants/instructions",
}
DEFAULT_LIMITS = {"trial_timeout_s": 900, "reference_max_steps": 40, "permissions": "full_auto", "max_retries": 2}
DEFAULT_REPEATS = {"pilot": 1, "formal": 3}
DEFAULT_ANALYSIS = {
    "metric": "checks_ok (all checks exit 0 in the trial workspace)",
    "pairing": "same repeat index within a task",
    "aggregate": "mean of per-task paired differences",
    "interval": "95% bootstrap over tasks, 10000 resamples, seed 20260924",
    "h1": "B vs A: no observed degradation; interval must not be significantly negative",
    "h2": "C vs B: reproducible gain; per-task positive on >=2 tasks and interval lower bound > 0",
    "insufficient": "interval contains 0 or too few samples -> report not confirmed",
}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("out", nargs="?", default="manifest.json")
    parser.add_argument("tasks", nargs="?", default="", help="comma-separated task ids (default: all in tasks/)")
    parser.add_argument("--design", default="review/eval/r2-p6/design.md")
    parser.add_argument("--groups", default=json.dumps(DEFAULT_GROUPS))
    parser.add_argument("--limits", default=json.dumps(DEFAULT_LIMITS))
    parser.add_argument("--repeats", default=json.dumps(DEFAULT_REPEATS))
    parser.add_argument("--analysis", default="", help="a JSON object, or the path to one")
    parser.add_argument("--note", default="")
    parser.add_argument("--config", default="", help="the frozen experiment config the batch writes")
    parser.add_argument("--task-timeout", action="append", default=[], metavar="ID=SECONDS",
                        help="a per-task wall-clock bound (round 5: the race tasks carry their own)")
    parser.add_argument("--frozen-on", default="2026-09-24")
    args = parser.parse_args()

    only = [name for name in args.tasks.split(",") if name] or None
    tasks = []
    for task_dir in sorted(p for p in (HERE / "tasks").iterdir() if p.is_dir()):
        if only and task_dir.name not in only:
            continue
        entry = {
            "id": task_dir.name,
            "prompt_sha256": hashlib.sha256((task_dir / "prompt.md").read_bytes()).hexdigest(),
            "checks": (task_dir / "checks.txt").read_text(encoding="utf-8").splitlines(),
            "fixture_sha256": digest_tree(task_dir / "fixture") if (task_dir / "fixture").is_dir() else None,
        }
        tasks.append(entry)
    if only:
        missing = [name for name in only if name not in {t["id"] for t in tasks}]
        if missing:
            print(f"no such task(s): {', '.join(missing)}", file=sys.stderr)
            return 2
    timeouts = {}
    for pair in args.task_timeout:
        task_id, _, seconds = pair.partition("=")
        if not seconds.isdigit():
            print(f"--task-timeout wants ID=SECONDS, got {pair!r}", file=sys.stderr)
            return 2
        if task_id not in {t["id"] for t in tasks}:
            print(f"--task-timeout names {task_id}, which is not in this manifest", file=sys.stderr)
            return 2
        timeouts[task_id] = int(seconds)
    for task in tasks:
        if task["id"] in timeouts:
            task["timeout_s"] = timeouts[task["id"]]

    analysis = DEFAULT_ANALYSIS
    if args.analysis:
        path = pathlib.Path(args.analysis)
        analysis = json.loads(path.read_text(encoding="utf-8") if path.exists() else args.analysis)
    analysis = dict(analysis)
    analyze = HERE / "analyze.py"
    analysis["sha256"] = hashlib.sha256(analyze.read_bytes()).hexdigest() if analyze.exists() else None

    manifest = {
        "frozen": args.frozen_on,
        "design": args.design,
        "groups": json.loads(args.groups),
        "model": {
            "key": "leader_main",
            "wire_model": "deepseek-flash",
            "context_window": 1000000,
            "context_source": "D-36: user-confirmed DeepSeek Flash native 1M (2026-09-17)",
            "reasoning_effort": "high",
            "effort_note": "catalog default max (~240s per simple turn); the pilot phase sets high explicitly to bound cost and duration",
        },
        "limits": json.loads(args.limits),
        "repeats": json.loads(args.repeats),
        "surface": surface_pin(),
        # Deliberate changes to the driver (`run.py`) after the pre-registration, each with the decision that
        # records it: `review/eval_manifests.py` compares the driver's syntax tree with the frozen bytes and
        # needs to know which differences are intended (D-145's rule; D-182 is the first entry).
        "driver_changes": [
            {"decision": "D-182", "what": "run-header.json also names the manifest the batch ran, and hashes it"},
        ],
        "analysis": analysis,
        "tasks": tasks,
    }
    if args.note:
        manifest["note"] = args.note
    if args.config:
        manifest["config"] = {
            "path": args.config,
            "sha256": hashlib.sha256((HERE / args.config).read_bytes()).hexdigest(),
        }
    manifest["git"] = subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip()
    (HERE / args.out).write_text(json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"manifest frozen: {len(tasks)} task(s) -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
