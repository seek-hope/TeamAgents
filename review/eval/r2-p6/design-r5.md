# R2-P6 round 5 pre-registration (the delegation race, frozen before any round-5 formal trial)

Written 2026-09-28, before the round-5 formal round ran. Basis: the four recorded rounds, round 4's pilot
(`design-r4.md`), the delegation-cost measurement D-256, and the two round-5 reconnaissance batches (`D-257`
without the timer rule, `D-258` with it). Everything below that names a number is fixed by those two batches.

## 1. What the earlier rounds left open

H1 passed (135/135) and **H2 was not confirmed**: the four recorded rounds measured the *propensity* to
collaborate (zero spawn/delegate in 99 group-C trials), not the *value* of collaborating. Round 4 added the
directive arm D and showed the propensity is reachable (2 workers and 2 delegated tasks in every pilot trial),
but its tasks were passed by both arms, so H4 could not be resolved on them. D-256 then measured *why* a
delegated trial is expensive (members hold no `shell@workspace`, so they cannot run their own acceptance checks
and the leader re-runs everything), and D-257's reconnaissance measured the race material itself and found the
arm's loss mode (a leader parked on task conditions with **no timer**, over members that ended their turn
without settling).

## 2. The treatment

| Group | Runtime | Collaboration surface | Instructions | The user's part |
|---|---|---|---|---|
| B | the persistent single instance (`v2::driver`) | none | `AGENT_INSTRUCTIONS` | — |
| D | the supervisor (`v2::supervisor`), the same manage/delegate/message grants as C | yes | `AGENT_INSTRUCTIONS` + the directive paragraph (D-254) | the harness issues `shell@workspace` to every member within 100 ms of its appearing (D-256) |

Both groups read the **same product tool text** (the `wait` / `delegate` / `spawn` descriptions, D-255/D-257/
D-258): those are contract text, not treatment, so they cannot be what separates the arms. D's treatment delta
over B is exactly: the collaboration surface, the directive paragraph, and the user's grant.

## 3. The race, its bound, and what makes a task a race task

**The calibration is a measurement, and one sample per arm is not one.** The two single-repeat batches
(`runs/2026-09-28-r5-recon/` without the timer rule, `runs/2026-09-28-r5-recon2/` with it) are *pilots*: they
showed the treatment change moved the arm's loss mode (900.2 s timeout -> 82.2 s, checks ok) and that B's own
wall clock on the same task spreads over 90.8-121.9 s, which is wider than the difference the criterion is
asked to resolve.

**Which candidate the race is expected on, and why** (D-256's arithmetic): the solo arm pays for every unit
(`B ~ units x unit time`), while the team arm pays once for the parallel phase plus the leader's integration
(`D ~ max(unit) + integration`), so the ratio grows with the *number of units* and not with their size. The
pilots agree: `six-deliverables` (6 units) measured 1.48x and `six-mixed` (6 units) did not reach it. The
**primary candidate is therefore `twelve-deliverables`** — the same twelve units as the two six-unit fixtures,
composed into one workspace — and the two six-unit tasks stay in the round as controls, with their pilot numbers
recorded as pilots.

So the calibration is `CALIB`: **3 repeats x {B, D} x the primary candidate**, and the criterion is stated on
the *median* of those three:

* a candidate is a **race task** iff `median(solo) >= 1.5 x median(team)` — the margin the bound below needs;
* a race task's bound is `bound = floor((median(solo) + median(team)) / 2)`, frozen in `manifest-r5.json` as the
  task's `timeout_s`;
* every other candidate stays a **control**, keeps the manifest's 900 s limit, and enters no hypothesis.

The threshold is unchanged from `D-257` (1.5x); what changed is the estimator, from one sample to the median of
three, and it changed *before* `CALIB` ran and before any formal trial. `CALIB` is recorded with its own batch
header, and its numbers are the only calibration input the formal round uses.

`TASKS` (filled from the two batches, see the round-5 section of `REPORT.md`): the race task(s) and their
bounds, and the controls.

## 4. Hypotheses and decision rules (pre-registered)

| # | Hypothesis | Decision rule |
|---|---|---|
| H4'a | on a race task, delegation turns a solo failure into a team success | in **>=2 of 3 repeats**: B's `checks_ok` is false **and** D's is true |
| H1' | the team arm does not regress on the controls | on each control task, D's `checks_ok` count over the 3 repeats is **>=** B's |
| H6 | cost and duration vary by group | real tokens and wall clock are reported per task and in total; **not** success thresholds |

A secondary, reported-but-not-decisive number: the pooled per-repeat paired difference (D − B) over all round-5
trials, with the 95 % bootstrap interval (10,000 resamples, the manifest's seed), computed by `analyze.py`. Too
few samples, a rule missed, or an interval containing 0 means "not confirmed" — never "equivalent".

## 5. Frozen parameters

`manifest-r5.json`: the four tasks with their frozen prompts, fixtures, checks and (for a race task) its bound;
model `leader_main` (wire `deepseek-flash`), native context 1,000,000 (D-36), `reasoning_effort = high`,
`full_auto`, `max_retries = 2`, a fresh working and state directory per trial, 3 repeats, the frozen experiment
config (`eval-config.toml`) in a private `XDG_CONFIG_HOME` per batch, and the surface pins recomputed from the
harness by `review/eval_surface.py`.

## 6. What the calibration found (written after `CALIB`, before any formal trial)

The calibration ran (`runs/2026-09-28-r5-calib12/`, 3 repeats x {B, D} x the primary candidate): B's median was
**102.7 s** (79.5-153.9) and D's **131.3 s** (92.7-732.2), both 3/3 accepted. The criterion asks for
`median(solo) >= 1.5 x median(team)`; the ratio is **0.78**, so **`twelve-deliverables` is not a race task** and
**no formal round is run** from this pre-registration. The two six-unit candidates were measured in the
pilots (133.5 s against 125.1 s, and 64.6 s against 85.0 s) and do not qualify either. Round 5 therefore ends in
the measured negative that `D-259` records, with the mechanism (both arms are flat in unit count and their
distributions overlap) rather than with a bound nobody could meet.

## 7. What round 5 does **not** establish

It stays on one model and one provider. It shows what a *run* of the design's flow pays when the user does their
part (the grant), not what the product's default pays. A wall-clock race is a claim about time, not about the
quality of the work. The race tasks reuse unit material the model has solved before (each unit is somebody's
earlier task), so they measure orchestration, not novel difficulty. And if only one candidate qualifies as a
race task, the claim generalises to that task alone; the report says so instead of pooling it away.
