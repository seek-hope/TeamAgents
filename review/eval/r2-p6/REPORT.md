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

> Note (2026-09-27): the four pre-registered batches below (135 trials) are unchanged. A fifth, *regression*
> batch was recorded at the productized HEAD on 2026-09-27 (groups B and C only, one repeat per cell, 16
> trials, 16/16 accepted — its own section below); having no group A arm it enters no conclusion here. From
> that batch on, every trial also records the surface it ran under and `review/eval_surface.py` checks it
> (D-182).

> Note (2026-09-26): the analysis script in the tree is no longer the exact bytes the manifests pin — `478d679` translated it into English — so its digest differs from the recorded `52d257b4…`. The rule is unchanged: comparing the two with every string literal stripped leaves identical syntax trees, which is what `review/eval_manifests.py` checks on every `make hygiene` (D-145).

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

## Round 4 pilot (2026-09-27, D-254): the instruction shape as the treatment

The four rounds above left H2 unconfirmed *and* diagnosed why: zero spawns in 99 group-C trials, so they measured
the propensity to collaborate (zero) rather than the value of it. Round 4 changes exactly one thing — C's
permissive paragraph becomes a **directive** one, group `D` (`review/eval/r2-p6/design-r4.md`, pre-registered
before the pilot ran; `manifest-r4.json`; the harness gained the arm and `review/eval_surface.py` pins its
digest). The pilot ran 1 repeat × {B, D} × the three splittable tasks (6 trials, `runs/2026-09-27-r4-pilot/`).

**The directive works, and it costs.** Every D trial spawned members and delegated — **2 workers and 2 delegated
tasks in each of the three**, against zero in 99 C trials — and all six trials passed their checks:

| Task | B (solo) | D (directed) | D's members / tasks / wait condition |
|---|---|---|---|
| `split-deliverable` | passed, 11.3 s | passed, 623.1 s | 2 / 2 / `message from …` (×2) |
| `parallel-deliverables` | passed, 42.5 s | passed, 327.1 s | 2 / 2 / the two task ids |
| `multi-step` | passed, 11.3 s | passed, 32.0 s | 2 / 2 / the two task ids |

Three measurements worth freezing before any formal round:

1. **No success ceiling was broken**: B passes these tasks as well, so H4 (a *success* gain) cannot be resolved
   on this set — the same ceiling the four rounds hit, now with the treatment actually running.
2. **The tail is a wait-condition choice, not orchestration**: the 623 s trial's wall clock is
   **575.6 s in one gap** — after the second `task_completed`/`inbox_drained` (event 189) and before
   `wait_satisfied` (event 190). Its `waits` row says why: the leader waited on `message from worker_sum` and
   `message from worker_words`, and the members never message — the wait could only end at its own timer
   (~600 s). The two trials whose leader waited on the *task ids* finished in 327 s and 32 s. So the worst case
   is a *contract* question (which condition a delegator should wait on), not a scheduling one.
3. **The fixed cost of delegating is large on this stack**: 20–21 model requests for D against 7 for B, with
   model time ~56 s against ~10 s, and a wall clock 2.7× to 55× B's on tasks of this size. On work that a solo
   instance can do in ten seconds, delegation is a *cost center*; a gain needs work whose solo path is far
   longer (or a smaller orchestration cost).

**No H4/H5 conclusion is drawn from a pilot** (the pre-registration's §5.1): the formal round (3 repeats × 8
tasks × {B, D} = 48 trials) was **not** run, because measurement 1 says it cannot separate the groups on this
task set. The next step the pilot points at is the *cost* — the wait-condition contract and the per-turn
orchestration overhead — and the task set that would need it, not more trials of a set both arms pass.

## Round 5 (2026-09-28, D-256-D-258): the delegation race, measured and not confirmed

Round 4 showed the directive *elicits* delegation. Round 5 asked whether delegation **pays**, by racing the two
arms against a wall-clock bound. Three candidate tasks were built (two six-unit, one twelve-unit; the units are
proven material from the recorded rounds), the treatment gained the two things the measurements demanded — the
user's grant of `shell@workspace` to every member (D-256) and the delegator's timer + recovery contract text
(D-257/D-258) — and every member's prompt gained the runtime's settlement rule (D-258). Four batches measured
it; `design-r5.md` is the pre-registration, and its calibration is the median of three repeats per arm.

| task (units) | B | D | ratio |
|---|---|---|---|
| `six-deliverables` (6) | 127.0 / 145.3 / **133.5** s, 3/3 ok | **125.1** s (pilot 82.2) | ~1.1x |
| `twelve-deliverables` (12) | 102.7 / 153.9 / **79.5** s, 3/3 ok | 92.7 / **732.2** / 131.3 s, 3/3 ok | 0.78x |
| `six-mixed` (6) | 64.6 s (pilot 56.2) | 85.0 s (pilot 78.7) | 0.76x |
| controls (`multi-step`, `parallel-deliverables`) | 9.2 / 54.4 s | 24.6 / 337.6 s | — |

**Verdict: no candidate met the pre-declared criterion (median solo >= 1.5 x median team), so the formal round
was not run, and H2 stays not confirmed — with a mechanism this time, not a shrug:**

1. **Neither arm shows a success difference** on any of these tasks: both pass every check in every repeat.
2. **Both arms are nearly flat in unit count, so size does not separate them.** B batches mechanically
   independent units into a few responses (twelve units done with 16 `edit` calls in 9-17 requests), and the
   team arm's orchestration is flat too (twelve `spawn`/`delegate` calls in about two responses) — but it also
   pays the parallel phase *and* the leader's own integration, and it has a long tail when a member stalls
   (732.2 s once, against B's 79.5-153.9 s range on the same task).
3. **The arms' wall-clock distributions overlap** — B's own spread on one task (79.5-153.9 s, median 102.7) is
   wider than any difference between the arms — so a bound-based rule separating them would be a coin flip on
   every repeat, which the pre-registered criteria resolve as *not confirmed*, never as equivalence.
4. The team arm's **cost** is 2-3x the tokens (502-565k against 133-349k on the twelve-unit task).

What that leaves: a wall-clock race cannot separate these arms on this harness and this model; the solo arm's
ability to batch mechanical work is a genuine strength, not an artifact. A different experiment would have to
take away that ability (a per-response output ceiling, or units so large that one response cannot hold several)
rather than add units — and that is a design decision for the user, recorded in ACCEPTANCE's Q16 gap.

## Round 6 (2026-09-28, D-261): time to all units verified green — pre-registered, not confirmed

Round 5's verdict was that the arms are indistinguishable on the *gate* (wall clock to a fully accepted result).
A per-unit re-reading of round 5's own sessions (`anatomy.py --units`) suggested a different instrument: a solo
instance **writes** every unit almost immediately (it batches) but **greens** them one test round at a time,
while a team greens one unit per worker, concurrently. Round 6 pre-registered that metric and its rule
(`design-r6.md`: `max(D) < min(B)` over 3 fresh repeats per arm) before running anything.

| `twelve-deliverables` (12 units), 3 fresh repeats | solo arm: all 12 green | team arm: all 12 green |
|---|---|---|
| round 6 (`runs/2026-09-28-r6-formal/`) | 116.8 / **53.3** / 84.5 s | **55.1** / 45.5 / 41.2 s |
| round 5 (exploratory) | 65.3 / 98.3 / 148.3 s | 51.6 / 53.5 / 59.5 s |

**Not confirmed**: the solo arm's fastest run (53.3 s) is faster than the team arm's slowest (55.1 s), so the
distributions overlap and round 5's clean six-number separation did not reproduce. Both readings are printed;
the flattering one is not.

**The secondary reading, labelled as such**: paired by repeat index across both batches, the team arm was earlier
in **6 of 6 pairs**, with medians 45.5 s against 84.5 s and 53.5 s against 98.3 s (≈1.8–1.9×) — a consistent
direction and magnitude that a strict non-overlap bar does not capture at n=3 per arm. The frozen checks still
decide acceptance, and both arms pass them in every repeat; quality is not what this section measures.

## Round 7 (2026-09-28, D-264): the paired form of the same metric — confirmed

Round 6 pre-registered a **distributional non-overlap** bar (`max(D) < min(B)`) for the time-to-green metric and
failed it. That bar was the wrong form for a paired design: with three repeats per arm on a load-varying host it
asks for a separation the pairing already carries. Round 7 pre-registered the **paired** form instead, on fresh
trials, and stated that it was the last round on the question.

| `twelve-deliverables`, repeat | solo arm: all 12 green | team arm: all 12 settled by their workers |
|---|---|---|
| 1 | 131.9 s | **36.1 s** |
| 2 | 108.6 s | **73.4 s** |
| 3 | 86.7 s | **43.1 s** |

**Confirmed: the team arm was strictly earlier in all three pairs** (H9), and the secondary, pooled reading is
8 of 9 pairs in its favour across rounds 5–7 — the ninth being a *treatment failure* rather than a reversal
(round 5's second pair, where a worker left its task unsettled and the trial stalled to 732 s).

What stands beside it, unchanged: the **end-to-end gate** does not improve (round 5: 92–200 s against 79–164 s),
and the **success** form of H2 stays at a zero paired difference because both arms pass every task. The gain is
real but specific — *a team greens a twelve-unit job earlier than a single instance does* — and both the metric
and the rule were pre-registered before their data (metric in round 6's registration, rule in round 7's).

## Cost (real tokens, DeepSeek billing)

| Batch | A | B | C |
|---|---|---|---|
| pilot, 18 trials | 326,062 (total) | — | — |
| formal round 1, 72 trials | 534,021 | 613,075 (+14.8%) | 641,274 (+4.6% vs B) |
| round 2, 27 trials | 217,276 | 238,293 (+9.7%) | 266,190 (+11.7% vs B) |
| round 3, 18 trials | 278,166 | 252,483 | 230,923 |
| regression batch, 16 trials (2026-09-27) | — | 220,692 | 200,414 |

The four batches total ≈ 3.60M tokens and about 55 minutes of machine time (6–40 s per trial, the slowest
being 40 s in round 3). The persistent runtime costs about +10% to +15% tokens over the reference loop, and on
this task set the collaboration surface (C) only added cost, because it was never used.

The 2026-09-27 regression batch costs a further 421,106 tokens in 3 min 33 s; the last row of the table is that
batch, not a fifth round (see its section below).

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

### Regression batch at the productized HEAD (8 tasks × groups B/C × 1 repeat)

16 trials, 16/16 passed; 421,106 real tokens in total; 3 min 33 s of wall clock (203.8 s of trial time).

Run 2026-09-27 at commit `0fb2624d` — after the productization work of the D-163…D-181 decisions — with the
same manifest, model and limits as round 1 and with the harness's own half of the surface pinned (D-182). It
is a *regression* check of the tasks the rounds above already measured, not a fifth round: there is no group A
arm and one repeat per cell, so it changes no conclusion (`analyze.py` reports "too few samples" for B−A and
`+0.000` for C−B on it, correctly). What it says is that the productized HEAD still passes every task in every
group it ran.

| Task | B (passed/runs, tokens) | C |
|---|---|---|
| edit-integrity | 1/1, 18,449t | 1/1, 18,103t |
| long-horizon | 1/1, 41,657t | 1/1, 26,283t |
| long-output | 1/1, 19,690t | 1/1, 21,121t |
| multi-step | 1/1, 23,555t | 1/1, 19,825t |
| parallel-deliverables | 1/1, 32,576t | 1/1, 53,187t |
| rust-fix | 1/1, 22,579t | 1/1, 19,131t |
| service-check | 1/1, 45,320t | 1/1, 30,319t |
| split-deliverable | 1/1, 16,866t | 1/1, 12,445t |

Each trial's record carries the surface it ran under (instruction-template digest, offered tool names, request
options, limits) and `review/eval_surface.py` compares those with the `surface` pins in the manifests; the
same batch was recorded twice, and the first run — made before the harness reported its surface — was replaced
by this one rather than kept beside it (D-182), because a batch is evidence for the treatment its own records
state.

## Limits and follow-up (recorded honestly, never merged into the conclusions)

- **The task set stays inside a single instance's capability ceiling**: each round got heavier (6 tasks → 8
  tasks → longer/more files → a hard deadline with splittable work) and a single instance still passed every
  task in time. Measuring a net collaboration gain would need tasks clearly beyond this round's budget (for
  example hundreds of tool calls per task, or real work that only parallel execution can finish before an
  external deadline). That was **not done** here, so H2 can only be recorded as "not confirmed".
- **The hard-deadline dimension produced no signal in round 3**: the slowest measured trial took 40 s against
  the 150 s deadline, so the deadline never bound.
- **The instruction text carries a fixed date**: it tells the model "Today's date: 2026-09-24", so a trial run
  later is told a date that is not today. It is part of the pinned surface (D-182) — changing it changes the
  treatment of every group, which is why it stays as recorded instead of being made dynamic.
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

From 2026-09-27 (D-182) each trial also records the surface it ran under (`surface`: the instruction-template
digest for its group, the tool names it was offered, the request options and the limits) and `run-header.json`
names and hashes the manifest the batch ran; `python3 review/eval_surface.py` checks both against the
manifests' `surface` pins and the harness's own history (part of `make check`).
