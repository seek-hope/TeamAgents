# R2-P6 pre-registration (frozen experiment design, written before any result)

Basis: §13 of the design baseline (13.1 local cost and model experiments stay separate, 13.2 samples and
integrity, 13.3 no unfounded numeric thresholds). This file was written **before** any trial ran; results are
analysed strictly against the criteria frozen here and never adjusted afterwards.

## 1. Question and hypotheses

| # | Hypothesis | Decision rule (pre-registered) |
|---|---|---|
| H1 | B (persistent single instance) shows **no observable regression** against A (the same kernel driven by a direct reference loop, without team management) | Per-task paired (B−A) success difference; the 95% bootstrap interval over the cross-task aggregate must not be significantly negative, and no systematic pattern of "B fails while A succeeds" may appear per task; otherwise the regression is reported as measured |
| H2 | C (B plus the visible collaboration surface) delivers a **reproducible gain across independent runs** over B | The per-task (C−B) paired success difference must be positive and consistent in ≥2 tasks, and the lower bound of the cross-task 95% bootstrap interval must be > 0; failing that, the report says "not confirmed" |
| H3 | Cost and duration vary by group | Per-task and cross-task real tokens and wall clock are reported but are **not** success thresholds |

- What A and B expose to the model is equivalent (same instructions, tool schemas, generation options, native
  window); C only adds the collaboration section and the spawn/delegate/message grants, which is the
  experimental treatment and is billed the same way (13.1).
- Too few samples, an interval containing 0 or excessive variance means "not confirmed" — never "equivalent"
  and never a claimed gain.

## 2. The three groups (mirrored by `engine/examples/eval_groups_abc.rs`)

| Group | Runtime | Collaboration surface |
|---|---|---|
| A | `reference::run_reference`, a direct loop with no persistence promise (evaluation-only) | none |
| B | the persistent single instance (`v2::driver`: one database, phase machine, runner-executed shell) | none |
| C | the supervisor (`v2::supervisor`, starting from a single instance) plus manage/delegate/message grants and collaboration instructions | yes |

All three share: `permissions = full_auto` (host shell, consistent with the evaluation environment, D-41),
`max_retries = 2`, a fresh working and state directory per trial, identical task prompts and acceptance
scripts, and the same `teamagents jobs-runner` behind the shell.

## 3. Frozen parameters

| Item | Value | Source |
|---|---|---|
| Catalog key | `leader_main` (wire `deepseek-flash`) | `~/.config/teamagents/config.toml` |
| Native context window | 1,000,000 | D-36 (user-confirmed; never shrunk) |
| Generation options | `reasoning_effort = high` (explicit override of the catalog default max) | this pre-registration; max takes ~240s on a simple turn, so high bounds cost and duration during the pilot |
| Per-trial timeout | 900 s | this pre-registration |
| Group A step limit | `max_steps = 40` | this pre-registration |
| Repeats | pilot: 1 per (task × group); formal: **3** per (task × group) | 13.2: the formal repeat count follows pilot variance and resolvability; this pre-registration uses 3 and states that limit in the report |
| Working directory | new per trial, fixture copied in | 13.2 |
| Acceptance | after the trial, each line of `checks.txt` runs in the **same** working directory; all exit codes 0 means success | 13.1 "the same real target environment" |

## 3b. Pilot outcome and the frozen formal set (2026-09-24, before the formal runs)

The pilot (6 tasks × 3 groups × 1 repeat = 18 trials) finished under the pre-registered criteria with
**18/18 acceptances**: 326,062 real tokens in about 15 minutes (6–25 s per trial). Conclusion: (1) cost and
duration are comfortably affordable (the formal three repeats are on the order of 1.0M tokens); (2) those 6
tasks show **no success variance** at one repeat (a ceiling effect), so H2's gain cannot be resolved under the
pre-registered criteria. Therefore **before running the formal batches** the formal set was extended to 8
tasks (two longer tasks added), which 13.2 allows as "fixing the formal set from the pilot", keeping the
repeat count at 3 and the original 6 tasks unchanged with H1/H2 criteria unchanged. The formal batch is
8 tasks × 3 groups × 3 repeats = 72 trials.

## 4. Task set (6 pilot tasks plus 2 formal additions, frozen)

| ID | Property covered | Description |
|---|---|---|
| `edit-integrity` | precise editing, semantic acceptance | change `retries` in the `[staging]` section only, leave every other section untouched |
| `long-output` | long command output, honesty | find a TOKEN inside a huge output and write `answer.txt` |
| `multi-step` | independent multi-step repair | two independent bugs in `alpha`/`beta`, each with its own check |
| `service-check` | environment/services, background processes | start an HTTP service, verify it with real curl, write the response body, stop the service |
| `split-deliverable` | splittable work (the C treatment) | two independent deliverables plus a one-line note |
| `rust-fix` | semantic acceptance (a real test suite) | fix the implementation behind a failing `cargo test` without touching the tests |
| `long-horizon` | long-horizon, multi-file, semantic acceptance | implement the `tools/` package so the frozen `tests/run_tests.py` passes (test hashes are verified) |
| `parallel-deliverables` | splittable (heavy), two independent acceptances | fix the `csvfix/` implementation and implement the `rules/` engine, each with pytest passing and test hashes verified |

Task properties and the sha256 of prompt/checks/fixture are recorded in `manifest.json` and frozen before the
run.

## 5. Analysis (frozen)

1. Record per trial: group, task, repeat index, status (A: completed/reply/failed; B/C: goal status/idle
   reply/timeout), `checks_ok`, real tokens, wall clock, failure class.
2. Per task × group: successes/repeats and the mean real tokens.
3. Pairs: for every task take the two groups' results at the same repeat and compute the success and token
   differences; aggregate across tasks by the mean.
4. Interval: **10,000 bootstrap resamples** over the per-task paired differences (seed = 20260924), reported
   as a 95% interval.
5. Decision: apply the criteria from section 1 for H1/H2; any interval containing 0 or too few samples means
   "not confirmed".
6. Failures are always classified (timeout / model error / acceptance failure / infrastructure); nothing is
   cherry-picked or re-run to look better, and exploratory extra runs are listed separately, never merged
   into the formal result.

## 6. Budget

Run the pilot first (6×3×1 = 18 trials), extrapolate the formal cost and duration from the measured tokens
and wall clock, and record both; over budget or over time is recorded as a failed trial — the criteria are
never adjusted to fit.

## 7. Report

`review/eval/r2-p6/runs/<date>/` (raw JSONL plus per-trial results) and `REPORT.md` (tables, paired analysis,
conclusions, re-run commands), mirrored into the P6 section of `docs/ACCEPTANCE.md`. Real and fake-service
results are kept apart, and nothing that was not run goes into the conclusions.
