# Terminal-Bench with TeamAgents (the benchmark's half)

`teamagents_agent.py` is a [harbor](https://github.com/harbor-framework/harbor) **installed-agent** adapter: it
copies the static `teamagents` binary and a frozen config into a Terminal-Bench task container and runs one
headless turn (`teamagents exec --json`) against the task instruction. Nothing here changes the product; it is
the harness's half of the run. The comparable baseline (D-375) and the protocol it comes from are in
`docs/DECISIONS.md`.

## How to run

```bash
# one-time: a venv with harbor, and the static product binary
python3 -m venv /tmp/harbor-venv && /tmp/harbor-venv/bin/pip install harbor
cargo build --release --offline --manifest-path engine/Cargo.toml \
    --target x86_64-unknown-linux-musl --bin teamagents      # rustup target add x86_64-unknown-linux-musl

# the model credential lives in the environment, never in the command or the repo
set -a; . ~/.config/teamagents-selfupdate/paratera.env; set +a

export PATH=/tmp/harbor-venv/bin:$PATH
TEAMAGENTS_TURN_TIMEOUT_SEC=840 PYTHONPATH=$PWD/review/benchmark \
  harbor run -d "terminal-bench/terminal-bench-2-1@latest" \
    --agent teamagents_agent:TeamAgentsAgent -m DeepSeek-V4.1-Flash \
    -y -n 2 -o /tmp/ta-harbor/jobs
```

`-i <task>` (repeatable) or `-l <n>` selects a subset; `harbor download "terminal-bench/terminal-bench-2-1@latest"`
puts the 89 tasks on disk for a fixed sample.

## The protocol this runs, and where it differs from the official one

The adapter freezes: paratera `DeepSeek-V4.1-Flash`, provider/protocol `deepseek`, native **1,000,000**-token
context (D-36), `reasoning_effort = "high"`, `max_retries = 3`, `timeout = 300` per request, and one headless
turn per task. `--cwd /app` (the task's own workspace) and `--full-auto` (the container *is* the sandbox;
`approved_scope` needs bubblewrap, which a task container cannot promise).

Known divergences from the DeepSeek technical report's Terminal-Bench 2.1 protocol (D-375):

* **Network is public by default.** The official protocol evaluates TB 2.1 *without network*. Harbor's
  `network_mode = "no-network"` needs its own egress-control sidecar, whose compose project failed to
  initialise here (`EGRESS_CONTROL_SIDECAR_IMAGE_NAME` unset on the teardown path), so the first batch ran with
  the harness default. A comparable run must fix that first.
* **No `max_steps = 500` cap.** The product's turn is bounded by wall clock and budget, not by a model-generation
  round count; the adapter's `TEAMAGENTS_TURN_TIMEOUT_SEC` (840 s here) is what bounds a turn.
* **`reasoning_effort = high`**, where the official numbers are at maximum effort.
* **N = 1** per task in the first batch (the official scaffold table uses N = 3).

`harbor run --print-config` and each trial's `result.json` record what actually ran.

## First batch (2026-09-30, measured)

Six tasks, one attempt each, harbor's own 900 s agent timeout per task, concurrency 2, **28 m 53 s**:

| Task | Reward | Why |
|---|---|---|
| `cobol-modernization` | **1.0** | — |
| `fix-git` | **1.0** | — |
| `prove-plus-comm` | **1.0** | — |
| `sqlite-db-truncate` | **1.0** | — |
| `adaptive-rejection-sampler` | 0.0 | our turn failed: `permanent model error: model stream: error decoding response body` (exit 1) |
| `cancel-async-tasks` | 0.0 | our turn hit its own 840 s deadline (exit 124) |

**4/6 = 0.667 on this sample.** For scale only, and not as a like-for-like comparison: the same model's official
Terminal-Bench 2.1 numbers are **90.6** (DeepSeek Harness Minimal), 90.3 (mini-SWE), 88.0 (Claude Code), 86.1
(Pi), 84.1 (Codex), and a third party's public harness reports 83.9.

Raw job output: `harbor view <jobs dir>`; per-trial `result.json` carries the exception and the verifier reward.
