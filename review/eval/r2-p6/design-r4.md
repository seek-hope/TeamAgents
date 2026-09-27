# R2-P6 round 4 pre-registration (the instruction shape, frozen before any round-4 trial)

Written 2026-09-27, before the round-4 pilot ran. Basis: §13 of the design baseline (as `design.md`), the four
recorded rounds' results, and what they measured about *why* H2 was not confirmed.

## 1. What the four recorded rounds left open, measured

H1 passed (135/135 acceptances; B did not regress against A) and **H2 was not confirmed**: the per-task paired
(C−B) success difference was always 0, and **zero of the 99 group-C trials spawned a worker or delegated a
task** — verified trial by trial in SQLite. Round 3 added a hard wall-clock bound (the harness's
`trial_timeout_s = 150`) and two timebox tasks; every group still passed 3/3, so the bound never bound.

The honest reading: on these tasks the agent-under-test *never chose* to delegate, whatever the task shape, so
the four rounds measured **the propensity to collaborate** (zero) and not **the value of collaborating**. The
design's own words leave that open — C "allows staying a team of one" — and ACCEPTANCE's known gap says the same
("what would close it is a task set (or an instruction shape) that makes delegation the shortest path, which is
an experiment to design").

## 2. Round 4: the instruction shape becomes the treatment

| Group | Runtime | Collaboration surface | Instruction shape |
|---|---|---|---|
| B | the persistent single instance (`v2::driver`) | none | `AGENT_INSTRUCTIONS` (unchanged) |
| D | the supervisor (`v2::supervisor`), same manage/delegate/message grants as C | yes | `AGENT_INSTRUCTIONS` + the **directive** paragraph |

Group D holds the runtime, the tools, the grants, the model and the tasks fixed against C and changes exactly one
thing: the collaboration paragraph. C's is permissive ("You **may** build a team … delegate only work that can
progress independently"); D's is a directive:

```
     - Build a team for this task. Split it into the independent parts the prompt describes and spawn one worker
instance per part **before** doing any of the work yourself; delegate each part with its own acceptance check,
then wait for the members and integrate their results. Work directly only on what cannot be split.
```

Both texts are pinned by `manifest-r4.json` and re-derived from the harness by `review/eval_surface.py`, so the
treatment cannot drift silently; C's batch findings stay valid because C's own text and digest are unchanged.

**Why this is the clean experiment**: it separates the two questions the four rounds conflated. H4 asks whether
the collaboration *mechanism* pays when the work is delegated; H5 measures whether the directive *causes*
delegation at all. If H5 holds and H4 does not, the conclusion is "the runtime's collaboration is not what was
missing, the task set was"; if H4 holds, ACCEPTANCE's gap closes; if H5 fails, the measured conclusion is that
no instruction shape in this family elicits delegation from this model on this set, which is itself the answer
the gap needs (and it is reported as such, not spun).

## 3. Hypotheses and decision rules (pre-registered)

| # | Hypothesis | Decision rule |
|---|---|---|
| H4 | D (directed collaboration) shows a **reproducible gain** over B | the per-task paired (D−B) success difference must be positive and consistent in ≥2 tasks, and the lower bound of the cross-task 95 % bootstrap interval (10,000 resamples, seed **20260927**, frozen here) must be > 0; failing that the report says "not confirmed" |
| H5 | the directive **elicits delegation** (the propensity arm) | in the formal round, ≥2/3 of D trials must show at least one `instance_created` beyond the leader **and** a non-empty `tasks` table, read from each trial's `session.sqlite`; a D trial that did not delegate is recorded as a **treatment failure** and still counts in H4's denominator (never dropped) |
| H6 | cost and duration vary by group | real tokens and wall clock are reported per task and in total, and are **not** success thresholds (unchanged from `design.md` §1) |

Too few samples, an interval containing 0, or excessive variance means "not confirmed" — never "equivalent",
never a claimed gain. No trial is retried, cherry-picked or re-classified after the fact.

## 4. Frozen parameters

Everything in `manifest-r4.json`: model `leader_main` (wire `deepseek-flash`), native context 1,000,000 (D-36),
`reasoning_effort = high`, `full_auto`, `max_retries = 2`, `trial_timeout_s = 900`, `reference_max_steps = 25`,
a fresh working and state directory per trial, the same acceptance scripts, and the same eight tasks as round 3
(prompts, fixtures and checks unchanged and hash-pinned). The batches report the surface each trial ran under and
`review/eval_manifests.py` / `review/eval_surface.py` hold the records to it.

## 5. Plan

1. **Pilot**: 1 repeat × {B, D} × the three tasks that are explicitly splittable (`split-deliverable`,
   `parallel-deliverables`, `multi-step`), i.e. 6 trials. Purpose: check that the treatment *elicits* delegation
   and that the tasks can separate the groups at all — never a conclusion.
2. **Formal**: 3 repeats × {B, D} × all eight tasks = 48 trials, run once, no retries, analysed against §3.
3. Record: `runs/<date>-r4-pilot/results.jsonl` (one directory per batch, as the other batches record) and a
   `runs/<date>-r4-formal/` for the formal round, each with `run-header.json` (surface + manifest + config
   digests) and the
   report section in `REPORT.md`, including the propensity counts (H5) beside the paired differences (H4).

## 6. What round 4 does *not* establish

It does not show that the *default* C instructions pay: if H4 holds it shows that collaboration pays **when the
work is delegated**, and the directive is then part of the treatment, not a neutral frame. It does not measure a
per-task ceiling (one repeat cannot), and it stays on one model and one provider.
