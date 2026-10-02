# What TeamAgents' Terminal-Bench failures are made of

This is the product-side reading of the corrected run (D-382): the cheap things the harness could be doing
wrong were fixed first, and only then were the failures classified. Everything below is evidence from the fixed
20-task sample (20 × 3, `deepseek-flash`, `reasoning_effort = low`, 890 s, official endpoint); the raw trial
dirs are the ones `review/benchmark/README.md` describes.

## 1. The measurement had to be corrected before the failures meant anything

Two harness defects were producing **false zeros**, and both are now fixed (D-382):

- The adapter never called `ensure_system_dependencies(environment, ("curl", "ca_certificates"))`, which every
  built-in harbor agent calls. An image without a CA bundle cannot verify TLS, so the *verifier's* own
  `apt-get install curl` / `uvx` failed, pytest never ran, and the trial scored 0 for the wrong reason. In one
  sample, **5 of 59 trials** had a verifier that never ran; with the call, `adaptive-rejection-sampler` — 0/3
  before — passed.
- The phase-scoped network policy added on top of the task trees set the **environment** baseline to
  `no-network`, which also governs agent setup; every task instead declares `allow_internet = true`, i.e.
  harbor's `PUBLIC`. The comparable configuration is the task as authored, so the pristine trees are now used.

| Configuration | Trials | Passed | Wilson 95 % |
|---|---|---|---|
| before the fixes (allowlist network, no CA install) | 60 | 38 (63.3 %) | [0.507, 0.744] |
| **after the fixes (task-authored network, CA install)** | **60** | **46 (76.7 %)** | **[0.646, 0.856]** |

Eleven trials moved to a pass: `build-pmars` 0/3 → 3/3, `kv-store-grpc` 0/3 → 3/3,
`configure-git-webserver` 1/3 → 3/3, `adaptive-rejection-sampler` 0/3 → 2/3, and others. The earlier number was
understated by ~13 points; the corrected one is the one to compare with the leaderboard rows in the README.

## 2. The failures that survive the fixes

| Task | Result | Mode | What the agent delivered | What the test wanted |
|---|---|---|---|---|
| `query-optimize` | 0/3 | completed, wrong result | `/app/sol.sql`: fast, small, one statement, DB untouched | output rows **exactly equal** to the golden query |
| `pytorch-model-cli` | 1/3 | completed, wrong result | `cli_tool`, `weights.json`, `prediction.txt` all present | the CLI's predicted digit correct |
| `headless-terminal` | 2/3 | completed, wrong result | `headless_terminal.py` imports; non-interactive and interactive command tests pass | every behavior in the spec, including the remaining edge case |
| `gcode-to-text` | 0/3 | timeout, no artifact | 474–581 committed events, no `/app/out.txt` | the decoded text in `/app/out.txt` |
| `qemu-alpine-ssh` | 0/3 | **environment rot** | — | verifier's `debian:bullseye` apt sources 404, so pytest is never installed |

The last row is not a TeamAgents result: `apt-get install curl` fails on that image's own sources today, so the
verifier cannot run at all. It should be excluded (or the image refreshed) before any comparison.

## 3. The product-side root cause: no proof before "done"

Every surviving non-timeout failure has the same shape — **the deliverable exists, is well-formed, and is
semantically wrong** — and each one was checkable by the agent itself, cheaply, with what was already in the
container:

- `query-optimize`: run `my-sql-query.sql` and `sol.sql` against the same database and compare the rows. The
  test does exactly that; the agent did not.
- `pytorch-model-cli`: run `./cli_tool weights.json <image>` and compare the printed digit with the known label.
- `headless-terminal`: exercise `HeadlessTerminal` against the behaviors listed in the instruction before
  declaring the implementation finished.

The turn produces artifacts and stops. `teamagents exec --json` reports `"verification": []` and
`"verification_path": null`: the product has a place for acceptance evidence and the harness supplies none, and
the goal carries no criteria for the leader to satisfy. So "done" means "the model said so", not "the claim was
checked". This is a **product capability gap**, not a harness gap, and it is the highest-value thing to fix.

The timeouts split into two kinds, and only one is about time:

- `gcode-to-text` ran 474–581 events and still had no artifact at 890 s — and at a 12-hour budget it *finished*
  and still failed (D-381). More time does not convert it.
- One `adaptive-rejection-sampler` trial committed **31 events in 16 minutes** while its siblings committed
  161 and 429. That is an early stall burning the window, which a stall detector could cut short and retry.

## 4. Proposed improvements, each with a falsifiable claim

Nothing here is implemented: product behavior changes are the user's call (AGENTS.md). Each proposal states the
claim, how it would be measured on this same harness, and what would be verified formally.

### P1 — Goal-level acceptance checks that must pass before "done"

The goal carries acceptance criteria (commands, or a shell predicate); the driver runs them before settling the
goal; a failure feeds the failure text back to the worker and the turn continues, bounded by the same budget.

- **Claim.** On tasks whose success is mechanically checkable, a turn with acceptance criteria converts
  completed-but-wrong into pass at a measurable rate, and does not lower the rate on tasks it cannot check.
- **Measurement.** The 20-task sample, N = 3, with and without `--accept`; report per-task flips. Predicted
  first flips: `query-optimize` (compare the two queries), `pytorch-model-cli` (run the CLI).
- **Formal.** `V2Goal`/`V2Retention` extension: a goal cannot reach `COMPLETED` unless every declared check is
  green, and a check failure never settles the goal. TLA+ model with a negative control that allows settling on
  red (refuted), plus a Kani harness for the "criteria parsed but empty" edge.

### P2 — A verifier teammate, not self-review

The leader spawns one member whose task is to reproduce the acceptance checks independently and report, so the
same context that produced the artifact does not grade it.

- **Claim.** For the class where the interface is right and the semantics are wrong, an independent verifier
  finds the mismatch more often than the producer does.
- **Measurement.** Same sample; count tasks where the verifier rejects an artifact that the producer declared
  done, and the flip rate when the rejection is fed back.
- **Formal.** The existing audience/visibility model already carries reviewer scoping; the new obligation is
  "no goal completes while a verifier that was asked has not reported" — a `V2Goal` invariant with a
  control that completes on a pending verifier.

### P3 — Detect a stalled turn and retry instead of burning the window

A turn that commits almost nothing for a long wall-clock slice (31 events in 16 minutes vs 429 for a sibling)
is stalled. Detect it, abort the provider stream, and retry the request once before consuming the budget.

- **Claim.** Stall detection turns the low-watermark timeout trials into either a pass or a fast, honest
  failure, and does not fire on turns that are merely thinking (the 429-event sibling of the same task).
- **Measurement.** Re-run the sample; report the wall clock and watermark distribution and the `end` reasons.
- **Formal.** `V2Codemode`/`V2Schedule` style: a watcher that may abort only when the watermark is unchanged
  across a window, with a control that aborts a progressing turn (refuted).

## 5. The A/B for P1's real mechanism (D-385/D-386): the gate works, the score does not move

P1 was investigated and turned out to be **already implemented** — `limits.required_checks`, the driver's repair
round and the BLOCKED settlement (A16/§8, `V2Checks.tla`) — with one thing missing: the headless entry point a CI
job or a benchmark runs could not attach checks to a goal (its `--check` runs *after* the turn). D-385 added that
ingress (`exec --accept ID=COMMAND`, control command `require_checks`). The A/B below measures what it buys.

Same tasks, same model, same budget (890 s, `low`, official route, pristine tasks, concurrency 3), checks derived
**only from each task's instruction**, 3 attempts per arm:

| Task | bare | `--accept` | What the check could see |
|---|---|---|---|
| `query-optimize` | 2/3 | 1/3 | the instruction's real criterion (the rewritten query must return the same rows) |
| `gcode-to-text` | 0/3 | 0/3 | only "`/app/out.txt` is non-empty" |
| `pytorch-model-cli` | 0/3 | 0/3 | interface + self-consistency only (the digit's correctness is not in the instruction) |
| **total** | **2/9** | **1/9** | |

**The mechanism is observable and correct; the pass rate is flat.** In the `query-optimize` arm the gate did
exactly its job — two of the three runs attached the check, failed it, spent the bounded repair rounds and settled
the goal **BLOCKED** (`end: failed`, `goal_status: BLOCKED`, the failing check named) where the bare arm could have
reported a wrong solution as finished. It just did not *converge*: the model could not repair the query inside the
remaining budget. On the other two tasks the instruction does not carry a machine-checkable criterion, so the
checks were weak and caught nothing (and in `gcode-to-text` all three runs timed out before claiming success, so
the check — which only ever verifies a *claimed success*, §8 — never ran at all). 9 trials per arm is far too
small to separate 1/9 from 2/9.

**What the feature is worth, honestly stated.** It does not raise this model's Terminal-Bench score. It converts a
silent wrong "done" into an explicit, named, bounded failure, which is what a user who *knows* their acceptance
criterion asked for and what the CLI could not express before. Whether it raises the score for a user whose check
is the real criterion depends on the model's ability to repair — which, on this evidence, is the binding
constraint again (D-381).

## 6. The nine tasks that never passed at N = 3, each diagnosed

D-389 left nine tasks at 0/3. They are not one phenomenon, and three of them were not the agent's fault at all.
Each was re-measured where a measurement could separate "needs more time" from "cannot do it": the class-1
experiments below ran the same tasks with **four times their declared agent budget**
(`--agent-timeout-multiplier 4`), because the declared budget is what the official protocol gives them.

### 6.1 Time vs capability (class 1)

| Task | 0/3 at the declared budget | At 4× the budget | Reading |
|---|---|---|---|
| `gcode-to-text` | no `/app/out.txt` | **finished**, wrote `TEXT SHOWN BY text.gcode` | **capability** — it stopped trying to decode and wrote a description of the task instead. (D-381's 12-hour run reached the same place at 1877 s) |
| `make-doom-for-mips` | no frame | **finished, 2 of 3 tests pass**: `frame.bmp` exists and matches the reference; only `test_vm_execution` fails on one missing stdout line | **nearly solved** — the hard part (a matching frame) is done; the remainder is narrow and not about time |
| `extract-moves-from-video` | no `/app/solution.txt` | still no `/app/solution.txt` | **structurally out of reach then** — D-392 built the missing image path and D-394 re-measured: the model *uses* `view_image` (5 calls in one instrumented attempt) and still does not finish, so the barrier that was structural is gone and a capability gap remains |
| `train-fasttext` | no usable model (`model.bin cannot be opened`) | **ran the full 4 h**, produced a model of the **right size** (`test_model_size` passes) at **accuracy 0.582 against a 0.62 threshold** | **time-adjacent near-miss** — four times the budget buys a valid model 4 points short, and the trial still ended on the deadline |

So the four split cleanly: one **capability** (`gcode-to-text`), one **nearly solved** (`make-doom-for-mips`), one **structurally blocked by a missing product capability** (`extract-moves-from-video`), and one **time-adjacent near-miss** (`train-fasttext`). None of them is "the model cannot do it" in the flat sense the raw 0/3 rows suggested.

`gcode-to-text` is the cleanest single result in this document: with four times the budget the agent **finished
and answered wrongly**, on a task whose answer is a single line. More time does not reach it.

`extract-moves-from-video` deserved its own sentence, because it was a **product** gap rather than a model one — and after D-392/D-394 it is neither: the image path exists, the model reads pictures through it, and the task still exceeds it:
the task is to transcribe the moves out of a **video** of a Zork session, and this product has **no path that
carries an image to the model** — the codemode `image()` helper accepts `data:` URLs but v2 has no image context
flow at all (the ceiling recorded in D-376, and the reason the helper refuses remote URLs). Every attempt
therefore has to OCR frames locally; none produced a file in 7,200 s.

### 6.2 Well-formed artifact, wrong semantics (class 2)

Both failures here are **deterministic** — the three N = 3 trials produced byte-identical wrong output — so they
are defects of the agent's pipeline, not sampling.

**`mteb-retrieve`.** The instruction: embed `/app/data.txt` with `BAAI/bge-small-zh-v1.5` at a pinned revision
via the installed `mteb`, take the **5th highest** cosine similarity to the query `"terminal-bench"`, write that
line. The delivered `/app/result.txt` (collected as an artifact) held

```
HumanEval: Benchmarking Python code generation via functional examples
```

while the expected line is `MTEB: Massive Text Embedding Benchmark`. The task's own reference solution shows what
is easy to miss: this model needs the **`task_name="SciFact"` and `prompt_type=query|passage` arguments** to
`encode()`, the ranking is `torch.topk(..., k=5).indices[0][4]`, and the reference even asserts that the
similarities are unique before trusting a 5th-place rank. The agent produced a line from the right corpus with
the wrong method.

**`pytorch-model-cli`.** The verifier runs the delivered CLI over the first 50 MNIST test images and compares to
a reference list. The agent's `cli_tool` passes the single provided image (`test_cli_tool_executable` expects
`2`) and fails `test_cli_tool_output` with **exactly the same 11 of 50** mismatches in all three trials
(`image 0 is 7, expected 2`; `image 18 is 1, expected 8`; …). Artifact collection shows why. The agent's own
`cli_tool.cpp` says, in a comment it wrote:

```
// nearest-neighbour resize, scaled to [0,1] and normalized with
// mean 0.1307 / std 0.3081.
```

and its `preprocess()` computes `(v/255 - 0.1307) / 0.3081`. The task's reference solution does **no
standardisation at all**: `load_image` takes the red channel of a 28×28 RGBA decode and stores `r / 255.0`.
The verifier's images are written from raw `ToTensor` values × 255, so the reference's convention is the one the
hardcoded expected classes were produced with, and the agent's extra standardisation changes 11 predictions.

**The instructive part is that the agent did verify — and measured the wrong thing.** It wrote
`eval_mnist.py`, which downloads the real MNIST test set, generates its own PNGs, and evaluates three
preprocessing variants (`v/255`, the standardised one, and raw) against the reference `model.py`, plus a
200-image cross-check of the compiled tool against the labels. That check passes for the standardised variant
too, because **accuracy cannot separate the two conventions** — the model is right on ~78 % of images either
way. It then confirmed the provided image still predicts `2`. Every check it ran was self-consistent with the
assumption under test, so none could falsify it. This is the sharpest instance yet of the point D-383 made in
the abstract: **a self-check authored from the same assumption is not verification**, and the only check that
would have caught it — predicting the same labels as the reference on the *verifier's* kind of image, not on
its own — is exactly the independent-observer role P2 proposed.

### 6.3 Excluded, not measured (class 3)

`qemu-alpine-ssh` and `qemu-startup` are scored 0 by their images' mirror rot, in every run:
`python3 review/benchmark/verifier_health.py <jobs dir>` reports both, 3 of 3 trials each, and exits 1 so the
fact cannot be ignored. They are excluded from the denominator (see the README's exclusion convention).

## 7. What is already ruled out

- **Effort tier**: `max` is worse than `low` on the fixed sample (D-380).
- **Wall clock**: a 12-hour budget leaves the score flat and lets the previously-zero tasks finish and fail
  anyway (D-381).
- **Two of the failures themselves**: `qemu-alpine-ssh` is mirror rot, not a result; `adaptive-rejection-sampler`
  and `build-pmars` were harness artifacts and now pass.
