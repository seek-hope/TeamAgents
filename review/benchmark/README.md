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

## Measured results (2026-10-01)

Three runs, all on the phase-scoped protocol above and all under harbor's own per-task agent timeout (900 s for
every task these runs touched). Intervals are Wilson 95 % over trials; they overlap, so the differences are not
significant on these samples.

| Run | Model route | Tasks × attempts | `reasoning_effort` | Turn budget | Trials | Passed | Wilson 95 % |
|---|---|---|---|---|---|---|---|
| fixed sample | paratera | 20 × 3 | `high` | 840 s | 60 | 37 (61.7 %) | [0.490, 0.729] |
| fixed sample | paratera | 20 × 3 | `max` | 890 s | 60 | 35 (58.3 %) | [0.457, 0.699] |
| fixed sample | **official** | 20 × 3 | **`low`** | 890 s | 60 | **38 (63.3 %)** | **[0.507, 0.744]** |
| **full set** | paratera | **89 × 1** | **`high`** | **890 s** | **89** | **48 (53.9 %)** | **[0.436, 0.639]** |

**Throughput, not reasoning depth, is the binding constraint — and lower effort is better here.** Raising
`reasoning_effort` from `high` to `max` (the card's setting) *lowered* the result (37 → 35), and lowering it to
`low` raised it (37 → 38) while cutting agent-side exceptions from ~30 to **13**. All intervals overlap, so none
of the three differences is significant on 60 trials; what is not marginal is the mechanism: a turn that hits
its deadline is `lifecycle: ACTIVE` — still working — and **all 36 timed-out trials in the full set are ACTIVE**,
with watermarks from 118 to 4,721 committed events. The agent is not stuck; it runs out of the 900 s task
window. Maximum effort makes each step slower, so fewer steps fit; `low` fits more.

The runs before the `low` row used the paratera relay (`llmapi.paratera.com`, model `DeepSeek-V4.1-Flash`); it
began answering `403 team_model_access_denied` for every request while this was being measured, so the adapter
now defaults to the **official** `https://api.deepseek.com` route (`deepseek-flash`), which is also the route the
model card's own numbers come from. The model route, key variable and effort are all knobs
(`TEAMAGENTS_MODEL`, `TEAMAGENTS_BASE_URL`, `TEAMAGENTS_API_KEY_ENV`, `TEAMAGENTS_REASONING_EFFORT`).

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
