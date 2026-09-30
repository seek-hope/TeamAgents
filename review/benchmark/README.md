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

## Measured results (2026-09-30/10-01)

**Fixed 20-task sample × 3 attempts, no-network, 60 trials: 37 passed (61.7 %), Wilson 95 % [0.490, 0.729].**
The task list is every fifth of the 48 tasks whose own agent timeout is 900 s, so the sample is fixed and
reproducible: `adaptive-rejection-sampler`, `build-pmars`, `chess-best-move`, `configure-git-webserver`,
`db-wal-recovery`, `fix-code-vulnerability`, `gcode-to-text`, `git-multibranch`, `headless-terminal`,
`kv-store-grpc`, `log-summary-date-ranges`, `merge-diff-arc-agi-task`, `multi-source-data-merger`,
`openssl-selfsigned-cert`, `polyglot-c-py`, `prove-plus-comm`, `pytorch-model-cli`, `qemu-alpine-ssh`,
`query-optimize`, `regex-log`.

| Result | Tasks |
|---|---|
| 3/3 | `fix-code-vulnerability`, `git-multibranch`, `headless-terminal`, `log-summary-date-ranges`, `merge-diff-arc-agi-task`, `multi-source-data-merger`, `openssl-selfsigned-cert`, `polyglot-c-py`, `prove-plus-comm`, `regex-log` |
| 2/3 | `chess-best-move`, `db-wal-recovery` |
| 1/3 | `configure-git-webserver`, `pytorch-model-cli`, `query-optimize` |
| 0/3 | `adaptive-rejection-sampler`, `build-pmars`, `gcode-to-text`, `kv-store-grpc`, `qemu-alpine-ssh` |

Agent-side failures (32 of 60 trials carried an exception), attributed:

| Cause | Trials | What it is |
|---|---|---|
| turn hit its own deadline | 24 | `end: "timeout"`, exit 124 — the turn budget (840 s), not a crash |
| model stream decode error | 3 | `permanent model error: model stream: error decoding response body` — a transport failure **after** visible output, which `providers::stream_failure_msg` makes permanent by design (no replay of a partially emitted turn) |
| other non-zero exits | 5 | boot/transport failures; each trial's `result.json` carries the message |

For scale only, not as a like-for-like comparison: the same model's official Terminal-Bench 2.1 numbers are
**90.6** (DeepSeek Harness Minimal), 90.3 (mini-SWE), 88.0 (Claude Code), 86.1 (Pi, v0.84.2), 84.1 (Codex), and a
third party's public harness reports 83.9. Differences that matter beyond the sample: their runs use maximum
reasoning effort and `max_steps = 500`, this one `high` effort and a wall-clock bound.

An earlier 6-task pilot (four easy 900 s tasks plus `adaptive-rejection-sampler` and `cancel-async-tasks`) scored
4/6 = 0.667 with the harness default (public) network; it is a pilot, not a row, and is superseded by the table
above.

Raw job output: `harbor view <jobs dir>`; per-trial `result.json` carries the exception, the verifier reward and
the agent command.
