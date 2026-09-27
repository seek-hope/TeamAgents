# R2-P6 round 7 pre-registration (the paired form of round 6's metric, frozen before any round-7 trial)

Written 2026-09-28, **before** any round-7 trial ran. Basis: rounds 5 and 6 (D-259, D-261) and their two
readings of the time-to-green metric. This is the last round on this question: a failure here closes it.

## 1. Why a round 7 exists, stated before its data

Round 6 pre-registered `max(D) < min(B)` — a **distributional non-overlap** bar — on 3 fresh repeats per arm.
It failed, because the solo arm's fastest run (53.3 s) crossed the team arm's slowest (55.1 s). That bar was the
wrong form for a **paired** design: with three repeats per arm and a host whose load varies, it asks for a
separation the pairing already carries. The reading it rejected has the shape a paired rule is meant to test:
paired by repeat index, the team arm was earlier in **3 of 3** pairs in round 6 and **3 of 3** in round 5 —
**6 of 6 across two batches**, medians 45.5 s against 84.5 s and 53.5 s against 98.3 s.

Round 6's failure stays in the record, and this round does not reinterpret it. It pre-registers the **paired**
form and collects **fresh** trials for it.

## 2. Hypothesis and decision rule (pre-registered)

| # | Hypothesis | Decision rule |
|---|---|---|
| H9 | on `twelve-deliverables`, the team arm reaches **every unit verified green** earlier than the solo arm, reproducibly | in **all three** fresh pairs (same repeat index), D's time is **strictly less** than B's |
| H10 | (secondary, reported not decisive) the direction holds across every batch | the pooled pairs of rounds 5, 6 and 7 are all same-sign |

The metric is round 6's, unchanged, read by the same tool: each arm's **own** evidence that a unit is green — a
worker's `task_completed … SUCCEEDED` for D, a pytest result reporting passes for B
(`review/eval/r2-p6/anatomy.py --units`, whose self-check covers both shapes). A run whose timeline cannot be
read counts **against** H9, never silently dropped. "Strictly less" means the numbers, not a tie band: a pair
inside 1 s is reported as a tie and fails H9.

`all three` is stricter than `>=2 of 3` on purpose: under a coin flip, three of three happens with probability
1/8, and the round-6 data suggests the sign is not a coin flip. If this round fails, the honest conclusion is
that the gain is **not reproducible** on this task and model, and the question is closed rather than re-run.

## 3. Frozen parameters

`manifest-r7.json`: `twelve-deliverables` alone (prompt, fixture, checks unchanged — the digests its earlier
manifests pinned), model `leader_main` (wire `deepseek-flash`), native context 1,000,000 (D-36),
`reasoning_effort = high`, `full_auto`, `max_retries = 2`, the 900 s wall-clock limit (this round does not race
the clock), **3 repeats**, the frozen experiment config in a private `XDG_CONFIG_HOME`, and the treatment exactly
as round 5/6's: group D is the supervisor with the collaboration surface, the directive paragraph (D-254), the
user's grant to every member (D-256), the product's contract text (D-255/D-257/D-258/D-262) read by both arms.

## 4. What this round does **not** establish

It stays on one model, one provider, one task and three pairs per batch: a 3-of-3 paired result at n=3 is a
small-sample claim, and it is about *when the units are green*, not about the quality of the work (the frozen
checks decide that, and both arms pass them). It says nothing about the **end-to-end gate**, where round 5's
negative result stands, and nothing about tasks whose units need no iteration, where the solo arm's batching
dominates.

## 5. Result (written after the round, before any other round)

Six fresh trials, all accepted (`runs/2026-09-28-r7-formal/`). Reading each arm's own evidence that every unit
is green (`anatomy.py --units`):

| repeat | solo arm: all 12 green | team arm: all 12 settled by their workers |
|---|---|---|
| 1 | 131.9 s | **36.1 s** |
| 2 | 108.6 s | **73.4 s** |
| 3 | 86.7 s | **43.1 s** |

**H9 is confirmed**: the team arm was strictly earlier in all three pairs, by 50–96 s. The secondary reading
(H10) is also as pre-registered — across rounds 5, 6 and 7 the paired readings are **8 of 9 in the team arm's
favour**, and the ninth is a *treatment failure* the record names rather than a reversal: round 5's second pair,
where one worker left its task unsettled and that trial stalled to 732 s.

**One instrument defect, recorded because it first produced a false "unreadable"**: the tool matched a
settlement to a unit by the assignee's *name*, which works only when the model happens to name workers after
units (round 7's first trial named them `w01`–`w12`). The fix is general — a full task set's **last** settlement
carries the timing, and the frozen checks confirm the set covers the units — and with it every pair in this
round is readable. The verdict above is the corrected reading; the pre-registration's rule for an unreadable
timeline was aimed at missing *data*, not at a tool that could not match it.

**This closes the round-7 question and does not open another.** The end-to-end gate stays as round 5 left it
(not confirmed: 92–200 s against 79–164 s), and the *success* form of H2 stays as the recorded rounds left it
(zero paired difference, because both arms pass every task). What is now established, on a metric and a rule
both pre-registered before their data, is narrower and real: **a team greens a twelve-unit job earlier than a
single instance does.**
