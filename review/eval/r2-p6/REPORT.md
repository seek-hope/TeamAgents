# R2-P6 performance experiment report

> Note (2026-09-25): the driver entry was called `engine/examples/rebuild_p6.rs` when this record was
> written; it was renamed to `engine/examples/eval_groups_abc.rs` during the naming cleanup (every other
> command is unchanged).

Date: 2026-09-24. Pre-registration: `review/eval/r2-p6/design.md`, `manifest.json` (first round, 8 tasks),
`manifest-r2.json` (second round, 3 tasks) and `manifest-r3.json` (third round, 2 tasks with a 150 s hard
deadline); analysis script `analyze.py` (its sha256 is recorded in each manifest and was frozen before the
run). Model: DeepSeek Flash (catalog key `leader_main`), **native context 1,000,000** (D-36),
`reasoning_effort = high` (explicitly overriding the catalog default max, see design.md §3), `full_auto`
permissions, a fresh working directory per trial and the acceptance script executed in that same directory
after the trial.

## Conclusions (per the pre-registered criteria)

| Hypothesis | Conclusion | Evidence |
|---|---|---|
| H1: B (persistent single instance) shows **no observable regression** against A (direct reference loop) | ✅ passed | 135 trials across four batches, **A/B/C all passed acceptance in every repeat of every task (135/135)**; the per-task paired success difference is always 0 and the 95% bootstrap interval is [0, 0] (no negative values) |
| H2: C (visible collaboration) delivers a **reproducible gain across independent runs** over B | ❌ **not confirmed** | also 135/135, with a per-task paired difference of always 0; and **zero spawn/delegate in the 99 group C trials of three rounds** (verified trial by trial in SQLite: `instance_created` equals the trial count and the `tasks` table stays empty) — the collaboration surface was available and its instructions visible, but the model chose a team of one every time. The design's definition of C explicitly "allows staying a team of one" |

In other words: **the persistent runtime (B) did not harm task success** (the first question P6 had to answer,
with 135 real runs as evidence), while **"a net gain from the system choosing to collaborate on its own" was
not confirmed on this task set** — not because collaboration failed, but because these tasks fit a single
instance's capability and attention budget, so the model had no reason to split work. This matches the
pre-registered criteria of §13.2/§16 ("too few samples means not confirmed"), but **it does not permit
claiming that collaboration paid off**.

## Cost (real tokens, DeepSeek billing)

| Batch | A | B | C |
|---|---|---|---|
| pilot, 18 trials | 326,062 (total) | — | — |
| formal round 1, 72 trials | 534,021 | 613,075 (+14.8%) | 641,274 (+4.6% vs B) |
| round 2, 27 trials | 217,276 | 238,293 (+9.7%) | 266,190 (+11.7% vs B) |
| round 3, 18 trials | 278,166 | 252,483 | 230,923 |

The four batches total ≈ 3.60M tokens and about 55 minutes of machine time (6–40 s per trial, the slowest
being 40 s in round 3). The persistent runtime costs about +10% to +15% tokens over the reference loop, and on
this task set the collaboration surface (C) only added cost, because it was never used.

### Pilot R25 (6 tasks × 3 groups × 1 repeat)

18 trials, 18/18 passed; 326,062 real tokens in total.

| Task | A (passed/runs, tokens) | B | C |
|---|---|---|---|
| edit-integrity | 1/1, 14,037t | 1/1, 11,577t | 1/1, 13,602t |
| long-output | 1/1, 14,567t | 1/1, 17,794t | 1/1, 19,629t |
| multi-step | 1/1, 10,810t | 1/1, 21,180t | 1/1, 24,149t |
| rust-fix | 1/1, 13,450t | 1/1, 14,570t | 1/1, 17,188t |
| service-check | 1/1, 31,717t | 1/1, 26,126t | 1/1, 26,960t |
| split-deliverable | 1/1, 18,812t | 1/1, 14,102t | 1/1, 15,792t |

### Formal R26 round 1 (8 tasks × 3 groups × 3 repeats)

72 trials, 72/72 passed; 1,788,370 real tokens in total.

| Task | A (passed/runs, tokens) | B | C |
|---|---|---|---|
| edit-integrity | 3/3, 40,508t | 3/3, 41,333t | 3/3, 47,964t |
| long-horizon | 3/3, 82,254t | 3/3, 108,856t | 3/3, 131,059t |
| long-output | 3/3, 38,249t | 3/3, 45,586t | 3/3, 57,060t |
| multi-step | 3/3, 52,339t | 3/3, 63,777t | 3/3, 68,489t |
| parallel-deliverables | 3/3, 91,861t | 3/3, 129,261t | 3/3, 115,146t |
| rust-fix | 3/3, 44,440t | 3/3, 46,394t | 3/3, 53,230t |
| service-check | 3/3, 140,719t | 3/3, 122,991t | 3/3, 112,214t |
| split-deliverable | 3/3, 43,651t | 3/3, 54,877t | 3/3, 56,112t |

### R26 round 2 (3 heavier tasks × 3 groups × 3 repeats)

27 trials, 27/27 passed; 721,759 real tokens in total.

| Task | A (passed/runs, tokens) | B | C |
|---|---|---|---|
| bulk-modules | 3/3, 74,438t | 3/3, 81,338t | 3/3, 95,554t |
| long-chain | 3/3, 106,970t | 3/3, 99,795t | 3/3, 119,220t |
| wide-audit | 3/3, 35,868t | 3/3, 57,160t | 3/3, 51,416t |

### R26 round 3 (splittable tasks with a 150 s hard deadline × 3 groups × 3 repeats)

18 trials, 18/18 passed; 761,572 real tokens in total.

| Task | A (passed/runs, tokens) | B | C |
|---|---|---|---|
| timebox-audit | 3/3, 219,905t | 3/3, 168,597t | 3/3, 114,301t |
| timebox-two-modules | 3/3, 58,261t | 3/3, 83,886t | 3/3, 116,622t |

## Limits and follow-up (recorded honestly, never merged into the conclusions)

- **The task set stays inside a single instance's capability ceiling**: each round got heavier (6 tasks → 8
  tasks → longer/more files → a hard deadline with splittable work) and a single instance still passed every
  task in time. Measuring a net collaboration gain would need tasks clearly beyond this round's budget (for
  example hundreds of tool calls per task, or real work that only parallel execution can finish before an
  external deadline). That was **not done** here, so H2 can only be recorded as "not confirmed".
- **The hard-deadline dimension produced no signal in round 3**: the slowest measured trial took 40 s against
  the 150 s deadline, so the deadline never bound.
- No Codex execution member took part (the current implementation has no such member type) and heterogeneous
  model performance was not measured (§13.1 lists that separately).

## Re-running

```bash
cargo build --offline --manifest-path engine/Cargo.toml --example eval_groups_abc
python3 review/eval/r2-p6/freeze.py manifest.json            # recompute the task summaries (frozen before a run)
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<new directory>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<new directory>
python3 review/eval/r2-p6/run.py --phase formal --manifest manifest-r2.json --out <new directory>
python3 review/eval/r2-p6/run.py --phase formal --manifest manifest-r3.json --out <new directory>
python3 review/eval/r2-p6/analyze.py <directory>/results.jsonl
```

Raw data: `review/eval/r2-p6/runs/<batch>/results.jsonl` (one line per trial with status, real tokens, wall
clock, per-check output and runner stderr) plus `run-header.json` in the same directory (the frozen
analysis/harness/git summary). Running inside the sandbox gets killed silently by resource limits (it happened
in two batches); the formal batches ran outside the sandbox, and `--resume` continues an interrupted batch.
