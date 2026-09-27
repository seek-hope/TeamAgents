# R2-P6 round 6 pre-registration (time to all units verified green, frozen before any round-6 trial)

Written 2026-09-28, **before** any round-6 trial ran. Basis: round 5's four batches and its verdict (D-259), plus
an **exploratory** re-analysis of those recorded trials (D-261) — which is why this round exists at all, and why
it is pre-registered rather than concluded from the re-analysis.

## 1. What round 5 left, and what the exploratory re-analysis saw

Round 5 measured the wall clock to a *fully accepted gate* and found the arms indistinguishable (B 102.7 s
median against D 131.3 s on the twelve-unit task; distributions overlapping). D-261 then read the *same*
committed sessions a third way, per unit: a solo instance **writes** every unit almost immediately (it batches
12 implementations into one or two responses) and only later **greens** them, one test round after another —
65.3 s, 98.3 s and 148.3 s in the three recorded runs — while a team **settles** each unit as a worker greens it
in parallel: 51.6 s, 53.5 s and 59.5 s. No overlap in those six numbers, and the mechanism is the batching
asymmetry: writing batches, *greening* does not.

That is exploratory evidence, gathered after the metric was chosen, so it cannot be the conclusion. This round
pre-registers the metric and tests it on **fresh** trials.

## 2. Hypothesis and decision rule (pre-registered)

| # | Hypothesis | Decision rule |
|---|---|---|
| H7 | on an independent-units task, the team arm reaches **every unit verified green** earlier than the solo arm | for 3 fresh repeats per arm on `twelve-deliverables`: **`max(D) < min(B)`** — the team arm's slowest run is faster than the solo arm's fastest |
| H8 | the end-to-end gate does **not** improve | reported, not decisive (round 5's verdict stands: `checks_ok` and wall clock to the gate did not separate) |

The metric is each arm's **own evidence that a unit is green**, read from the committed session by
`review/eval/r2-p6/anatomy.py --units`:
* the team arm's is a unit's `task_completed` with `SUCCEEDED` — a worker that ran its own unit's tests;
* the solo arm's is a pytest result reporting passes and no failures, covering the units the call named.

Both are the same kind of event (a green run made by whoever did the work); the asymmetry is deliberate and is
the thing being measured — one worker per unit against one instance batching the runs. The instrument, the task
and the unit names are fixed here; the trials are fresh; a run whose unit timeline cannot be read is reported as
such and counts against the hypothesis, never silently dropped.

## 3. Frozen parameters

`manifest-r6.json`: the task `twelve-deliverables` (prompt, fixture and checks unchanged from round 5, whose
digests its manifests pinned), model `leader_main` (wire `deepseek-flash`), native context 1,000,000 (D-36),
`reasoning_effort = high`, `full_auto`, `max_retries = 2`, the round-5 wall-clock limit (900 s — this round does
not race the clock), 3 repeats, the frozen experiment config (`eval-config.toml`) in a private
`XDG_CONFIG_HOME`, and the treatment exactly as round 5's: group D is the supervisor with the collaboration
surface, the directive paragraph (D-254), the user's grant of `shell@workspace` to every member (D-256), and the
product's contract text (D-255/D-257/D-258) that both arms read.

## 4. What this round does **not** establish

It stays on one model and one provider. It measures *when the work is green*, not its quality (the frozen checks
still decide acceptance, and both arms pass them). It says nothing about tasks whose units need no iteration —
there, the solo arm's batching is the whole story and round 5's negative result stands. And one task, three
repeats: a non-overlap at n=3 is a small-sample claim, reported as such.
