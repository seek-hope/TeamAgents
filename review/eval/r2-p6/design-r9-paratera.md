# R2-P6 round 9 pre-registration (the configured model's capability pilot, frozen before its batch)

Written 2026-09-30. One smoke trial (`split-deliverable`, group B) had already run by hand to confirm the
endpoint speaks the configured protocol (status `succeeded`, 17 s, 17,094 tokens); **no other round-9 trial had
run** when this was written, and the batch below is run exactly as this document states it.

## 1. Why this round exists

Rounds 1–8 measured *relative* questions — B vs A (H1), C vs B (H2), D vs B (H11) — with the recorded
`deepseek-flash` profile. D-372's refresh says the comparison against Codex CLI and Pi is a *surface*
comparison and that no comparator is benchmarked. This round measures the complementary thing this repository
can measure with the model the user actually configured (paratera `DeepSeek-V4.1-Flash`, native window
1,000,000): the product's **absolute** pass rate on its own machine-checked tasks.

It exists because "is the product at the level of X" cannot be answered by a feature table (D-372), and the
honest half of the answer that *can* be measured here is "with model M, on tasks with objective checks, the
product greened N of M".

## 2. What is measured (no hypothesis, and no threshold)

| What | How |
|---|---|
| Primary | per-task `checks_ok` for **group B** (the product's persistent single-agent path) on all 16 frozen tasks, 1 repeat |
| Secondary | the same for **group D** (the team path, the supervisor with the directive instruction shape) on `split-deliverable` and `twelve-deliverables`, 1 repeat — an exercise of the team path so the round is not blind to it, **not** a speed comparison |
| Reported with each trial | status, steps, real tokens, wall clock, and a failing check's own output |

There is **no success threshold and no decision rule**: the reading is a number ("N of M tasks green") with its
cost beside it. A round with a threshold would need repeats and a pre-registered effect size; this is a pilot
and §4 says what that costs.

## 3. Frozen parameters

`manifest-r9-paratera.json`: all 16 tasks in `tasks/`; model key `leader_main` (wire `DeepSeek-V4.1-Flash`,
base URL `https://llmapi.paratera.com/v1`); native context **1,000,000** (D-36, never shrunk); `reasoning_effort
= high`; `full_auto`; `max_retries = 2`; 900 s per trial; **1 repeat**; the frozen experiment config
(`eval-config-paratera.toml`) written into a private `XDG_CONFIG_HOME` per batch (D-254). **No per-response
ceiling**: the recorded rounds' 2048-token treatment belongs to round 8's question and would only bound what
this round measures.

## 4. What this round does **not** establish

- It is **not** a comparison with Codex CLI or Pi: the tasks are this repository's own, not a shared benchmark,
  and no comparator was run. It is the number a comparator would have to be measured against on *these* tasks.
- It is **not** a statistical claim: 1 repeat per cell, so one flaky task moves the headline. The report says
  so, and a formal repeat count would be a new pre-registration.
- It is a capability reading of the product **under this model**, not a model-independent product quality.
- A trial that errors for a harness reason (`status = harness_error`, or a turn that fails on transport) is
  reported as recorded; nothing is retried and nothing is cherry-picked.

## 5. Results (written after the batch)

- **Group B, all 16 tasks: 16/16 green** (`runs/2026-09-30-paratera-capability`), 981,787 real tokens, 1,088 s
  of trial wall clock; per-task tokens 16,828–281,313 and steps 5–17.
- **Group D, `split-deliverable` and `twelve-deliverables`: 2/2 green**
  (`runs/2026-09-30-paratera-team`), 550,660 tokens, 318 s.
- **Deviations: none.** Both batches ran exactly as §3 states: no ceiling, the 900 s bound (the slowest trial
  used 252 s), nothing retried and nothing dropped. The one pre-written smoke trial in the preamble is part of
  neither batch.
