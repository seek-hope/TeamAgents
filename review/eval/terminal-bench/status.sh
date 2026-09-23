#!/usr/bin/env bash
# 查看一次 Terminal-Bench 运行的结果：总体统计 + 逐任务 reward/异常/JSONL 行数。
# 用法：review/eval/terminal-bench/status.sh [JOB_DIR]
set -uo pipefail
job=${1:-$(ls -dt "${JOBS_DIR:-$(cd "$(dirname "$0")" && pwd)/jobs}"/*/ 2>/dev/null | head -1)}
[ -n "$job" ] && [ -d "$job" ] || { echo "找不到 job 目录" >&2; exit 2; }
echo "JOB: $job"
python3 - "$job" <<'PY'
import glob, json, os, sys

job = sys.argv[1]
with open(os.path.join(job, "result.json"), encoding="utf-8") as handle:
    stats = json.load(handle)["stats"]
print(
    "total={} done={} errors={} running={} pending={}".format(
        sum(stats[k] for k in ("n_completed_trials", "n_errored_trials", "n_running_trials", "n_pending_trials")),
        stats["n_completed_trials"],
        stats["n_errored_trials"],
        stats["n_running_trials"],
        stats["n_pending_trials"],
    )
)
rows = []
for trial in sorted(glob.glob(os.path.join(job, "*__*"))):
    name = os.path.basename(trial).split("__")[0]
    log = os.path.join(trial, "agent", "teamagents.jsonl")
    lines = sum(1 for _ in open(log)) if os.path.exists(log) else 0
    result_path = os.path.join(trial, "result.json")
    if os.path.exists(result_path):
        with open(result_path, encoding="utf-8") as handle:
            result = json.load(handle)
        rewards = ((result.get("verifier_result") or {}).get("rewards") or {})
        rows.append(
            (
                name,
                rewards.get("reward"),
                (result.get("exception_info") or {}).get("exception_type"),
                lines,
            )
        )
    else:
        rows.append((name, None, "running", lines))
for name, reward, error, lines in rows:
    state = "running" if reward is None and error == "running" else f"reward={reward} error={error}"
    print(f"{name:40s} {state:28s} jsonl={lines}")
scored = [r[1] for r in rows if r[1] is not None]
if scored:
    print(f"mean reward: {sum(scored)/len(scored):.3f} over {len(scored)} finished")
PY
