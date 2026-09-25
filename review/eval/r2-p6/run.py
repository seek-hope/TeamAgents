#!/usr/bin/env python3
"""R2-P6 trial runner: one task × one group × N repeats, frozen by manifest.json.

    python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/2026-09-24-pilot
    python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/2026-09-24-formal

Per trial: a fresh workdir (fixture copied in) -> `eval_groups_abc --group G ...` -> each line of
checks.txt run with `sh -c` in that same workdir -> the result appended to `results.jsonl`. Failures are
classified and recorded; nothing is retried and nothing is cherry-picked.
"""
import argparse, hashlib, json, os, pathlib, shutil, subprocess, sys, time

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent.parent
BIN = REPO / "engine/target/debug/examples/eval_groups_abc"
TASKS = HERE / "tasks"


def load_manifest(name: str = "manifest.json") -> dict:
    return json.loads((HERE / name).read_text(encoding="utf-8"))


def run_checks(workdir: pathlib.Path, checks: list[str]) -> tuple[bool, list[dict]]:
    log = []
    ok = True
    for line in checks:
        proc = subprocess.run(["sh", "-c", line], cwd=workdir, capture_output=True, text=True, timeout=300)
        entry = {"check": line, "rc": proc.returncode,
                 "stdout": proc.stdout[-2000:], "stderr": proc.stderr[-2000:]}
        log.append(entry)
        if proc.returncode != 0:
            ok = False
    return ok, log


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--phase", choices=["pilot", "formal"], required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--only", default="")
    ap.add_argument("--groups", default="A,B,C")
    ap.add_argument("--repeats", type=int, default=0, help="override the manifest's repeat count")
    ap.add_argument("--resume", action="store_true", help="skip trials already present in results.jsonl")
    ap.add_argument("--manifest", default="manifest.json")
    args = ap.parse_args()

    manifest = load_manifest(args.manifest)
    repeats = args.repeats or (1 if args.phase == "pilot" else manifest["repeats"]["formal"])
    groups = [g for g in args.groups.split(",") if g]
    tasks = [t for t in manifest["tasks"] if not args.only or t["id"] == args.only]
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    results_path = out / "results.jsonl"
    completed = set()
    if args.resume and results_path.exists():
        for line in results_path.read_text(encoding="utf-8").splitlines():
            if line.strip():
                record = json.loads(line)
                completed.add((record["task"], record["group"], record["repeat"]))
        print(f"resume: {len(completed)} trial(s) already recorded", flush=True)
    if results_path.exists() and args.phase == "formal" and not args.resume:
        print("formal results already exist; use a new directory, or --resume to continue this batch", file=sys.stderr)
        return 2
    header = {
        "phase": args.phase, "repeats": repeats, "groups": groups,
        "started": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "manifest_analysis_sha256": manifest["analysis"]["sha256"],
        "harness_sha256": hashlib.sha256(BIN.read_bytes()).hexdigest() if BIN.exists() else None,
        "git": subprocess.run(["git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip(),
    }
    (out / "run-header.json").write_text(json.dumps(header, ensure_ascii=False, indent=2), encoding="utf-8")
    total = len(tasks) * len(groups) * repeats
    done = 0
    for task in tasks:
        checks = [line for line in (TASKS / task["id"] / "checks.txt").read_text(encoding="utf-8").splitlines() if line.strip()]
        for group in groups:
            for repeat in range(1, repeats + 1):
                done += 1
                tag = f'{task["id"]}.{group}.{repeat}'
                if (task["id"], group, repeat) in completed:
                    print(f"[{done}/{total}] {tag} skipped (already recorded)", flush=True)
                    continue
                workdir = out / "work" / tag
                state = out / "state" / tag
                if workdir.exists():
                    shutil.rmtree(workdir)
                if state.exists():
                    shutil.rmtree(state)
                workdir.mkdir(parents=True)
                fixture = TASKS / task["id"] / "fixture"
                if fixture.is_dir():
                    shutil.copytree(fixture, workdir, dirs_exist_ok=True)
                result_file = out / f"{tag}.json"
                cmd = [str(BIN), "--group", group, "--task-file", str(TASKS / task["id"] / "prompt.md"),
                       "--workdir", str(workdir), "--state", str(state), "--out", str(result_file),
                       "--id", task["id"], "--model", manifest["model"]["key"],
                       "--timeout", str(manifest["limits"]["trial_timeout_s"]),
                       "--max-steps", str(manifest["limits"]["reference_max_steps"])]
                started = time.time()
                proc = subprocess.run(cmd, cwd=REPO, capture_output=True, text=True,
                                      timeout=manifest["limits"]["trial_timeout_s"] + 300)
                wall = time.time() - started
                record = {"task": task["id"], "group": group, "repeat": repeat, "wall_s": round(wall, 1),
                          "runner_rc": proc.returncode, "runner_stderr": proc.stderr[-2000:]}
                if result_file.exists():
                    record.update(json.loads(result_file.read_text(encoding="utf-8")))
                else:
                    record["status"] = "harness_error"
                try:
                    ok, log = run_checks(workdir, checks)
                except subprocess.TimeoutExpired:
                    ok, log = False, [{"check": "timeout", "rc": 124}]
                record["checks_ok"] = ok
                record["checks"] = log
                with results_path.open("a", encoding="utf-8") as sink:
                    sink.write(json.dumps(record, ensure_ascii=False) + "\n")
                print(f'[{done}/{total}] {tag} status={record.get("status")} checks={"ok" if ok else "FAIL"} '
                      f'tokens={record.get("total_tokens")} wall={record["wall_s"]}s', flush=True)
    print(f"done: {results_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
