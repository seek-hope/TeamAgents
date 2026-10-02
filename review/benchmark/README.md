# Terminal-Bench with TeamAgents (the benchmark's half)

`teamagents_agent.py` is a [harbor](https://github.com/harbor-framework/harbor) **installed-agent** adapter: it
copies the static `teamagents` binary and a frozen config into a Terminal-Bench task container and runs one
headless turn (`teamagents exec --json`) against the task instruction. `patch_phases.py` gives a task the
phase-scoped network policy the official protocol implies. Nothing here changes the product; it is the harness's
half of the run. The comparable baseline (D-375) and the protocol it comes from are in `docs/DECISIONS.md`.

## How to run

```bash
# one-time: a venv with harbor, the compose/buildx CLI plugins, and the static product binary
python3 -m venv /tmp/harbor-venv && /tmp/harbor-venv/bin/pip install harbor
rustup target add x86_64-unknown-linux-musl
cargo build --release --offline --manifest-path engine/Cargo.toml \
    --target x86_64-unknown-linux-musl --bin teamagents
harbor download "terminal-bench/terminal-bench-2-1@latest" -o /tmp/tb-ds   # the 89 tasks

# the model credential lives in the environment, never in the command or the repo
set -a; . ~/.config/teamagents-selfupdate/paratera.env; set +a

export PATH=/tmp/harbor-venv/bin:$PATH DOCKER_BUILDKIT=1   # harbor's egress sidecar needs BuildKit (+buildx)
python3 review/benchmark/patch_phases.py /tmp/tb-ds/terminal-bench-2-1
TEAMAGENTS_TURN_TIMEOUT_SEC=840 PYTHONPATH=$PWD/review/benchmark \
  harbor run -p /tmp/tb-ds/terminal-bench-2-1 \
    --agent teamagents_agent:TeamAgentsAgent -m DeepSeek-V4.1-Flash \
    -y -n 6 -k 3 -o /tmp/ta-harbor/jobs
```

`-i <task>` selects a named subset (`harbor run -p <dir> -i a -i b …`), `-l <n>` takes the first n, `-k <n>` is
the attempts per trial. `harbor view <jobs dir>` browses runs.

## The protocol, and the one correction that matters

The adapter freezes: paratera `DeepSeek-V4.1-Flash`, provider/protocol `deepseek`, native **1,000,000**-token
context (D-36), `reasoning_effort = "high"`, `max_retries = 3`, `timeout = 300` per request, one headless turn
per task, `--cwd /app`, `--full-auto` (the container *is* the sandbox; `approved_scope` needs bubblewrap, which a
task container cannot promise).

**Network is phase-scoped, and that is what `patch_phases.py` writes:**

| Phase | Policy | Why |
|---|---|---|
| `environment` | `no-network` | the solution gets no network — the official TB 2.1 condition |
| `agent` | `allowlist` [`llmapi.paratera.com`] | only the model API stays reachable during the agent phase |
| `verifier` | `public` | the benchmark's own `tests/test.sh` installs pytest/uv; that setup is not the solution's network |

This correction is not cosmetic. A first no-network attempt put `no-network` on the environment baseline alone,
which also cut the **verifier** off: every verifier then died on `apt-get install curl` / `uvx: command not
found` and scored 0 whatever the agent had done. That batch (20 tasks × 3, 0/59 recorded) is **void** and is kept
here only as the reason the phases are explicit. The table below is the run whose verifiers actually executed
(checked: no verifier stdout contains `Unable to locate package curl`).

Remaining divergences from the DeepSeek card's protocol, recorded rather than hidden: `reasoning_effort` is
`high` where the card uses maximum effort; the product has no `max_steps = 500` counterpart, so a turn is bounded
by `TEAMAGENTS_TURN_TIMEOUT_SEC` (840 s here, under harbor's 900 s agent timeout) and by the goal's budget.

## The image path on the task that motivated it (D-392/D-394)

`TEAMAGENTS_IMAGES=1` declares image support for the run's model, which offers `view_image` and lets a picture reach
the wire. Measured on `extract-moves-from-video` — the task D-391 called structurally unreachable because it needs
the moves transcribed from a *video* — at the task's own 1800-second budget, three attempts: **0/3, all on the
deadline, still no `/app/solution.txt`**. An instrumented fourth attempt collected its workspace and session
database: the agent **called `view_image` five times**, ran 28 shell calls (extracting frames, trying OCR) and made
34 requests for 442k prompt tokens before the deadline. So the capability is used in a real task and does not
convert this one — the structural barrier is gone and a capability gap remains. The pre-D-392 baseline is in
`CAPABILITY.md` section 6.

## Verifier health: the trials that must be excluded

A scored 0 is only about the agent when the verifier actually ran. On two task images it cannot: `test.sh`
installs its own tooling and the image's `apt` sources 404 today, so `pytest` never starts. Those trials are
0 for a reason the agent cannot influence, and they have to be named rather than counted.

`python3 review/benchmark/verifier_health.py JOBS_DIR [...]` reads every trial's verifier stdout and reports
which tasks were never evaluated (it exits 1 when any were, so a comparison cannot ignore the fact). On the
three full-set runs to date it reports the same two tasks, every time:

```
/tmp/ta-harbor/jobs_full_n3: 267 trial(s), 6 whose verifier never ran, 89 task(s)
  EXCLUDE qemu-alpine-ssh: 3 of 3 trial(s) never evaluated
  EXCLUDE qemu-startup: 3 of 3 trial(s) never evaluated
```

**Exclusion convention from here on:** `qemu-alpine-ssh` and `qemu-startup` are scored 0 by their images'
mirror rot, not by TeamAgents, so they are **excluded from the denominator** of any headline figure and listed
as such beside it. The N = 3 figure is therefore **207/264 = 78.4 % [73.1, 82.9]** on the tasks whose verifier
ran (or **210/264 = 79.5 % [74.3, 84.0]** with the D-390 parser correction); the two tasks stay pending until
their images are refreshed.

## Measured results (2026-10-01)

Three runs, all on the phase-scoped protocol above and all under harbor's own per-task agent timeout (900 s for
every task these runs touched). Intervals are Wilson 95 % over trials; they overlap, so the differences are not
significant on these samples.

| Run | Model route | Tasks × attempts | `reasoning_effort` | Agent budget | Trials | Passed | Wilson 95 % |
|---|---|---|---|---|---|---|---|
| fixed sample | paratera | 20 × 3 | `high` | 840 s | 60 | 37 (61.7 %) | [0.490, 0.729] |
| fixed sample | paratera | 20 × 3 | `max` | 890 s | 60 | 35 (58.3 %) | [0.457, 0.699] |
| fixed sample | official | 20 × 3 | `low` | 890 s | 60 | 38 (63.3 %) | [0.507, 0.744] |
| fixed sample **(corrected harness, D-382)** | official | 20 × 3 | `low` | 890 s | 60 | **46 (76.7 %)** | **[0.646, 0.856]** |
| fixed sample | official | 20 × 3 | `low` | **12 h** | 59 | 38 (64.4 %) | [0.517, 0.754] |
| **full set** | paratera | **89 × 1** | **`high`** | **890 s** | **89** | **48 (53.9 %)** | **[0.436, 0.639]** |
| **full set (corrected harness, D-382)** | official | **89 × 1** | **`low`** | **890 s** | **89** | **67 (75.3 %)** | **[0.654, 0.831]** |
| **full set (per-task budgets, D-387)** | official | **89 × 1** | **`low`** | **task's own** | **89** | **68 (76.4 %)** | **[0.666, 0.840]** |
| **full set (per-task budgets, D-387)** | official | **89 × 3** | **`low`** | **task's own** | **267** | **207 (77.5 %)** | **[0.722, 0.821]** |
| full set **(after the D-390 parser fix** for one task) | official | 89 × 3 | `low` | task's own | 267 | **210 (78.7 %)** | **[0.733, 0.831]** |

The two full-set rows are **not** a controlled comparison: the corrected one differs in the harness fixes, the
model route (official instead of the paratera relay) and the effort tier at the same time.

**A single full-set row is a point estimate with a ±5-task band (D-388).**

**The N = 3 run is the headline row (D-389).**

**One row was measuring the CLI, not the model (D-390).** `pytorch-model-recovery`'s instruction begins with
`"- "`, and without an end-of-options marker the parser read it as an unknown flag: all three trials exited 2
**before the model was reached**. `exec` takes `--` now; a re-run of that task alone under the fixed binary
scored **3/3**, so the corrected figure is **210/267 = 78.7 % [73.3, 83.1]** — computed by replacing that task's
0/3 with its re-measured 3/3, which is sound because it is the only instruction in the dataset that starts
with a dash.
 207 of 267 trials passed, and because every task ran three
times the per-task mean agrees with the trial mean exactly: **77.5 % [72.2 %, 82.1 %]**. Per task: **55 tasks
3/3, 17 tasks 2/3, 8 tasks 1/3, 9 tasks 0/3**; 40 trials (15 %) ended in an exception (33 `AgentTimeoutError`,
7 non-zero exits), and 6 trials (`qemu-alpine-ssh` ×3, `qemu-startup` ×3) had a verifier that never ran
because those images' `apt` sources 404 today — so that number is a floor by up to two tasks. The nine tasks
that never passed in three attempts are `extract-moves-from-video`, `gcode-to-text`, `make-doom-for-mips`,
`mteb-retrieve`, `pytorch-model-cli`, `pytorch-model-recovery`, `qemu-alpine-ssh`, `qemu-startup` and
`train-fasttext`.
 The flat-890 s and per-task-budget
runs above used the same model, effort and route and moved **9 gains against 8 losses** for a net +1; split by
whether the budget really changed, the 50 tasks whose budget stayed ≤ 900 s went 39 → 34 (a swing with no
treatment difference, so it is the noise floor) while the 39 tasks whose budget was raised went 28 → 34. The
Wilson intervals above measure the sample, not that run-to-run band, which is why the official protocol's N = 3
is the shape a headline number needs — an N = 3 full set under the per-task budgets is in flight.
 The controlled
comparison is the fixed sample above (harness only: 38 → 46 of 60). Two of the 89 trials in the corrected row
(`qemu-alpine-ssh`, `qemu-startup`) had a verifier that never ran — their images' `apt` sources 404 today — so
that number is a floor by up to two tasks.

**The first and third rows used a harness that was understating the score.** Two defects (D-382) produced false
zeros: the adapter never installed `curl`/`ca-certificates`, so a verifier on an image without a CA bundle could
not `apt-get` or `uvx` and pytest never ran (5 of 59 trials); and the phase-scoped network had set the
*environment* baseline to `no-network`, which also governs setup, while every task declares
`allow_internet = true` (harbor's `PUBLIC`). With both fixed and the tasks used as authored, the same sample at
the same settings is **46/60 (76.7 %)**. Product-side reading of what still fails, the three proposals, and the A/B of the acceptance gate that
was implemented (D-385/D-386: its checks are observable and it does **not** raise the pass rate from 2/9 to more
than 1/9 — it converts a silent wrong "done" into a named, bounded failure): [CAPABILITY.md](CAPABILITY.md).

**A 12-hour budget does not raise the score: the failures are capability, not the clock.** Raising the agent
budget from 890 s to 12 h (and harbor's per-task timeout with it) left the fixed sample where it was — 38/60 at
890 s, 38/59 at 12 h, intervals overlapping — and only **7 of 59** trials used more than 900 s at all (median
wall 275 s, longest 2,317 s). The 890 s run's 36 `lifecycle: "ACTIVE"` deadline failures were the *visible*
symptom, not the cause: a targeted re-run of the six tasks that were 0/3 under the budget finished five of them
in 422–1,877 s with **no agent-side exception**, and every one still failed its own tests; only `kv-store-grpc`
recovered (1/3 at 12 h). So the honest reading of these rows is a **capability** measure of this model in this
product on these tasks, not a harness artifact. (The earlier wording here called the constraint "throughput";
that was wrong, and this paragraph replaces it.)

The 12 h row is a **test-time-compute point**, not the leaderboard protocol: the official protocol keeps each
task's own timeout (900 s–12,000 s here). `patch_phases.py` writes that cap with `TEAMAGENTS_AGENT_TIMEOUT_SEC`.

The runs before the `low` row used the paratera relay (`llmapi.paratera.com`, model `DeepSeek-V4.1-Flash`); it
began answering `403 team_model_access_denied` for every request while this was being measured, so the adapter
now defaults to the **official** `https://api.deepseek.com` route (`deepseek-flash`), which is also the route the
model card's own numbers come from. The model route, key variable, effort and budget are all knobs
(`TEAMAGENTS_MODEL`, `TEAMAGENTS_BASE_URL`, `TEAMAGENTS_API_KEY_ENV`, `TEAMAGENTS_REASONING_EFFORT`,
`TEAMAGENTS_AGENT_TIMEOUT_SEC`).

### The fixed 20-task sample (3 attempts each)

Every fifth of the 48 tasks whose own agent timeout is 900 s, fixed and reproducible:
`adaptive-rejection-sampler`, `build-pmars`, `chess-best-move`, `configure-git-webserver`, `db-wal-recovery`,
`fix-code-vulnerability`, `gcode-to-text`, `git-multibranch`, `headless-terminal`, `kv-store-grpc`,
`log-summary-date-ranges`, `merge-diff-arc-agi-task`, `multi-source-data-merger`, `openssl-selfsigned-cert`,
`polyglot-c-py`, `prove-plus-comm`, `pytorch-model-cli`, `qemu-alpine-ssh`, `query-optimize`, `regex-log`.

At `high` effort: 3/3 — `fix-code-vulnerability`, `git-multibranch`, `headless-terminal`,
`log-summary-date-ranges`, `merge-diff-arc-agi-task`, `multi-source-data-merger`, `openssl-selfsigned-cert`,
`polyglot-c-py`, `prove-plus-comm`, `regex-log`; 2/3 — `chess-best-move`, `db-wal-recovery`; 1/3 —
`configure-git-webserver`, `pytorch-model-cli`, `query-optimize`; 0/3 — `adaptive-rejection-sampler`,
`build-pmars`, `gcode-to-text`, `kv-store-grpc`, `qemu-alpine-ssh`.

### The full set (89 tasks, 1 attempt each)

48 of 89 passed. The tasks that did **not** pass are dominated by the wall-clock bound and by heavy environments:
`adaptive-rejection-sampler`, `build-cython-ext`, `build-pmars`, `build-pov-ray`, `caffe-cifar-10`,
`code-from-image`, `compile-compcert`, `configure-git-webserver`, `count-dataset-tokens`, `dna-assembly`,
`dna-insert`, `extract-moves-from-video`, `filter-js-from-html`, `financial-document-processor`, `gcode-to-text`,
`gpt2-codegolf`, `hf-model-inference`, `install-windows-3.11`, `kv-store-grpc`, `largest-eigenval`,
`llm-inference-batching-scheduler`, `make-doom-for-mips`, `mcmc-sampling-stan`, `mteb-retrieve`,
`nginx-request-logging`, `protein-assembly`, `pytorch-model-cli`, `pytorch-model-recovery`, `qemu-alpine-ssh`,
`raman-fitting`, `rstan-to-pystan`, `sam-cell-seg`, `sqlite-with-gcov`, `torch-tensor-parallelism`,
`train-fasttext`, `tune-mjcf`, `video-processing`, `winning-avg-corewars`. Four failed with **no** agent
exception (`extract-elf`, `mteb-leaderboard`, `qemu-startup`, `pytorch-model-recovery`) — the turn completed and
the task's own tests did not pass.

### Agent-side failures, attributed

| Cause | Full set (of 89) | What it is |
|---|---|---|
| turn hit its own deadline | 36 | `end: "timeout"`, exit 124 — the turn budget, not a crash |
| model stream decode error | 4 | `permanent model error: model stream: error decoding response body` — a transport failure **after** visible output, which `providers::stream_failure_msg` makes permanent by design (no replay of a partially emitted turn) |
| other non-zero exits | 12 | boot/transport failures; each trial's `result.json` carries the message |

37 of 89 trials finished with no agent-side exception at all.

For scale only, not as a like-for-like comparison: the same model's official Terminal-Bench 2.1 numbers are
**90.6** (DeepSeek Harness Minimal), 90.3 (mini-SWE), 88.0 (Claude Code), 86.1 (Pi, v0.84.2), 84.1 (Codex), and a
third party's public harness reports 83.9. Beyond the sample size, their runs allow `max_steps = 500` and give
the agent the whole agent timeout; the product has no step counter, and here the turn budget (890 s) is what
bounds it — which the attribution above says is the binding constraint.

Raw job output: `harbor view <jobs dir>`; per-trial `result.json` carries the exception, the verifier reward and
the agent command.
