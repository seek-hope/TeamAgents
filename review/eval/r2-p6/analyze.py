#!/usr/bin/env python3
"""R2-P6 analysis: per-task paired differences + pre-registered verdicts.

    python3 review/eval/r2-p6/analyze.py review/eval/r2-p6/runs/<date>/results.jsonl

口径完全按 design.md/manifest.json：成功=checks_ok；配对取同一次重复；跨任务聚合为均值；
区间=10,000 次对任务层面配对差的 bootstrap（seed=20260924）；区间含 0 或样本不足 → 「未证实」。
"""
import json, random, sys, pathlib
from collections import defaultdict

SEED = 20260924
RESAMPLES = 10000


def load(path):
    trials = []
    for line in pathlib.Path(path).read_text(encoding="utf-8").splitlines():
        if line.strip():
            trials.append(json.loads(line))
    return trials


def bootstrap_ci(values, seed=SEED, resamples=RESAMPLES):
    if not values:
        return None, None
    rng = random.Random(seed)
    n = len(values)
    means = []
    for _ in range(resamples):
        sample = [values[rng.randrange(n)] for _ in range(n)]
        means.append(sum(sample) / n)
    means.sort()
    return means[int(0.025 * resamples)], means[int(0.975 * resamples) - 1]


def main() -> int:
    trials = load(sys.argv[1])
    tasks = sorted({t["task"] for t in trials})
    groups = sorted({t["group"] for t in trials})
    by_task_group = defaultdict(list)
    for t in trials:
        by_task_group[(t["task"], t["group"])].append(t)
    print(f"{'task':<18}" + "".join(f"{g:>16}" for g in groups))
    for task in tasks:
        row = f"{task:<18}"
        for group in groups:
            runs = sorted(by_task_group[(task, group)], key=lambda r: r["repeat"])
            if not runs:
                row += f"{'-':>16}"
                continue
            wins = sum(1 for r in runs if r.get("checks_ok"))
            tokens = sum(r.get("total_tokens") or 0 for r in runs)
            row += f"{wins}/{len(runs)} ok {tokens}t".rjust(16)
        print(row)

    def paired(left, right):
        diffs, detail = [], {}
        for task in tasks:
            l = {r["repeat"]: r for r in by_task_group[(task, left)]}
            r = {r["repeat"]: r for r in by_task_group[(task, right)]}
            shared = sorted(set(l) & set(r))
            if not shared:
                continue
            success = []
            for repeat in shared:
                success.append((1 if l[repeat].get("checks_ok") else 0) - (1 if r[repeat].get("checks_ok") else 0))
            diff = sum(success) / len(success)
            detail[task] = diff
            diffs.append(diff)
        return diffs, detail

    for left, right in (("B", "A"), ("C", "B")):
        diffs, detail = paired(left, right)
        if not diffs:
            print(f"{left}-{right}: 样本不足 → 未证实")
            continue
        mean = sum(diffs) / len(diffs)
        low, high = bootstrap_ci(diffs)
        positive = sum(1 for value in detail.values() if value > 0)
        print(f"{left}-{right}: 配对成功差均值 {mean:+.3f}（逐任务 {detail}），"
              f"95% bootstrap 区间 [{low:+.3f}, {high:+.3f}]，正向任务 {positive}/{len(detail)}")
        if left == "B" and right == "A":
            if low < 0:
                print("  H1：区间下界为负 → 观察到退化，按设计报告")
            else:
                print("  H1：未观察到退化 ✅")
        if left == "C" and right == "B":
            if low > 0 and positive >= 2:
                print("  H2：跨任务可复现收益 ✅")
            else:
                print("  H2：未证实（区间含 0 或正向任务不足）")
    token_summary = defaultdict(int)
    for t in trials:
        token_summary[t["group"]] += t.get("total_tokens") or 0
    print("真实 tokens 合计:", dict(token_summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
