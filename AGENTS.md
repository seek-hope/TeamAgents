# TeamAgents repository conventions (binding for every agent working in this repo)

## Baseline and deviations

- The design and acceptance baseline is `docs/DESIGN.md` (requirements Q1–Q19, architecture and protocol
  constraints, the A01–A36 acceptance matrix and the definition of done). Per-item evidence and known gaps
  are in `docs/ACCEPTANCE.md`; currently binding decisions and still-valid earlier rules are in
  `docs/DECISIONS.md`.
- **Any implementation that differs from the confirmed design (even a simpler or better one) must be
  discussed with the user and confirmed before it goes into code.** Confirmed deviations are recorded in
  `docs/DECISIONS.md`; unconfirmed ones stay in discussion.
- This repository is a standalone Rust project: all implementation code is Rust (the `core`, `engine` and
  `tui` crates). `tui/scripts/*.py` and the Python fake servers inside some Rust tests are test-only.
- The repository carries only the current implementation: material from earlier implementations and their
  migration was removed from the tree (it stays reachable through Git history, see `git log -- docs/archive`).

## Language (repository-wide)

- **Code is English only**: identifiers, comments, doc comments, error messages, help text, user-facing
  strings, scripts, workflows. No Chinese anywhere in code.
- **Documentation is English only**, with exactly one exception: `README.zh-CN.md`, the Chinese README.
  Commit messages are English too. `make language-check` (part of `make hygiene`) enforces this on the whole
  tracked tree, so a stray non-English character fails the gate instead of being caught in review.
- One deliberate exception inside the code scan: the frozen evaluation material under
  `review/eval/r2-p6/tasks/**` (task prompts and fixtures) and the recorded trial output under
  `review/eval/r2-p6/runs/**` keep their original bytes, because the manifests pin their hashes and they are
  evidence of runs that already happened. Changing them would require re-freezing the manifests and re-running
  the experiments.

## Quick commands

The single development entry point and its maintenance rules are in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).
The toolchain is pinned in `rust-toolchain.toml`; run `make check` before committing (offline by default),
`make fmt` to fix formatting, and `make pty` for the isolated real-terminal check. The first run may need
`make check CARGO_FLAGS=--locked` to download dependencies.

```bash
cargo test --offline --manifest-path core/Cargo.toml    # authoritative core (control plane + kernel + specs)
cargo test --offline --manifest-path engine/Cargo.toml  # engine (phase machine, supervisor, daemon, jobs, MCP)
cargo test --offline --manifest-path tui/Cargo.toml     # TUI logic + TestBackend frames
engine/target/debug/teamagents {init,doctor,daemon,exec,version}  # CLI entry points
engine/target/debug/teamagents exec --json --timeout 180 "…"      # headless turn (starts the daemon if needed)
make pty                                 # real-terminal smoke check (tui/scripts/pty_v2_smoke.py)
make check-nobwrap                       # `make check` with no bwrap in PATH: the CI runner's condition (D-113)
make check-broken-sandbox                # `make check` with a bwrap that cannot start a sandbox (Ubuntu 24.04, D-114)
make verify-model-all                    # small TLA+ configurations for all modules (see verification/README.md)
make verify-kani                         # Kani proofs (paging arithmetic)
python3 review/eval/r2-p6/run.py --phase pilot --out <new date directory>  # real-model A/B/C comparison (needs credentials)
```

- Baseline (2026-09-27): `make check` is green — core 101 / engine 251 / tui 35. Raw evaluation JSONL lives
  in `review/eval/r2-p6/runs/`, per-item evidence in `docs/ACCEPTANCE.md`.
- Skipped checks and the current baseline are collected in `docs/ACCEPTANCE.md`. A green Cargo run is not a
  real-service acceptance result.
- Real-model evaluation always uses the model's native context window and records the value and its source;
  never shrink a window for a real-model test. DeepSeek Flash uses the user-confirmed 1M configuration.
  Tests run with a non-native window are invalid, must be deleted rather than renamed as stress experiments.

## Architecture at a glance (read this before changing code)

This describes the current code.

- Single team-transaction entry point: `core/src/v2/control.rs::Control::submit` (ingest → validate →
  reduce → persist, one SQLite transaction per session; identity, operation ids and permission revisions are
  filled in by the control plane, never taken from a model or client field).
- Authoritative state: `core/src/v2/store.rs` (WAL plus explicit `synchronous=FULL`; a format stamp refuses
  foreign or wrong-version databases). In-process callers serialize through the bounded single-writer worker
  in `engine/src/v2/storage.rs`.
- Execution: `engine/src/v2/driver.rs` (phase machine; model and tool waits happen outside transactions) and
  `engine/src/v2/supervisor.rs` (one coordinator per state root driving every ACTIVE instance). Shell
  commands run in the runner process of `engine/src/jobs`.
- Tools and bindings: `engine/src/tools.rs` (files, shell, web), `engine/src/bound.rs` (binding is the
  authorization), `engine/src/mcp.rs` (stdio and streamable HTTP). User hooks live in `engine/src/hooks.rs`
  (`[hooks]`: `pre_tool` veto, `notify` events).
- Information permissions: visibility and delivery decisions in `core/src/v2/control.rs` plus the context
  views in `core/src/kernel/*` (`audience` visibility is not `push` delivery; observers get scoped payloads).
- Product layer: `engine/src/v2/daemon.rs` (one Unix-socket JSON-lines service per state root),
  `engine/src/v2/exec.rs` (headless client), `engine/src/cli.rs`, `tui/src/daemon_client.rs`. Rendering and
  mouse hit-testing share `tui/src/v2ui.rs::geometry`.
- Workspace policies: `engine/src/workspace.rs` (shared / isolated / git worktree, §5.1) is selected by the
  `workspace` argument of the `spawn` tool. `driver::prepare_spawn_workspace` resolves the policy before the
  child instance starts and records it in `<instances_dir>/<id>/workspace.json`; the supervisor retires that
  workspace when the instance is `TERMINATED` (it refuses to delete anything with uncommitted or unmerged
  work and reports why).

## Reviews and evidence (`review/*`)

- Current evidence: `docs/ACCEPTANCE.md` (per item A01–A36), `verification/REPORT.md` (formal-verification
  results, evidence, unproven list) and whatever `review/README.md` lists as current.
- Older review and evaluation batches from earlier implementations are no longer in the tree; they are
  reachable through Git history (`git log -- review/archive`) and do not describe the current code.
- Read-only reviews must not modify the reviewed files, and every conclusion needs a re-runnable command or
  probe (probes go to /tmp or `review/tmp/`). Before claiming a falsification, rule out probe error.

## Operating notes for team runs (measured)

- **Cancelling a task is not stopping its work.** `tasks cancel` lands the task `CANCELLED` and releases
  the delegator, but the assignee's running command keeps going until its own tool timeout; the lever that
  stops the process group is `instances terminate --id … --yes` (measured: the effect stops 1.5–6 s after
  the lever, receipt `class: cancelled` — D-88, `python3 review/dogfood/cancel.py`).
- **A task can park in `BLOCKED`** when required checks are exhausted, a member fails or work is cancelled;
  a blocked task prevents the goal from completing. Select it in the TUI tasks panel and press `c` to cancel
  it (without a live turn it goes straight to `CANCELLED`; headlessly: `teamagents tasks cancel --id`);
  re-dispatch blocked work as a new task.
- **A member that stops talking**: a turn whose model replies with plain text (no tool call) and does not
  settle its task now ends the member's activity with the task still `RUNNING` (D-65), instead of the runtime
  asking again forever. The delegator's wait stays pending — cancel the task to satisfy it (`c` in the tasks
  panel, or `teamagents tasks cancel --id` headlessly, D-68), or send the member another instruction. A
  `BLOCKED` task does not satisfy a delegator's wait; `CANCELLED` does.
- **Avoid interrupting a member turn**: keep tasks small and acceptance criteria explicit. After an
  interruption recovery is driven by persisted location — known results are reused, in-flight losses are
  recorded as `OUTCOME_UNKNOWN`, and side effects are never replayed on a guess.
- **Approvals** bind one concrete operation, its argument hash and its permission revision, so an approval is
  used up by that single dispatch. The PENDING approval of an operation that settles or an instance that
  terminates is expired automatically; a leftover can be denied with `d` in the approvals panel.
- **Transport failures**: a read timeout or truncated stream after the response headers is retried inside
  the turn up to `max_retries` as long as no visible text was emitted; once text went out (or the protocol
  itself failed) the attempt fails immediately and the reason lands in the receipt
  (`transient retries exhausted` / `permanent model error` / `context overflow`).
- **Goal deadline and budget**: a goal may carry a `deadline` and a usage ceiling; past the deadline no new
  request starts, and usage is settled honestly.
- **"Write early" for heavy tasks**: a turn is bounded by budget and wall clock, so a read-many/write-report
  task should drop artifacts on disk along the way instead of ending with uncommitted conclusions.

## Code style

- Lazy-first: standard library over existing dependencies over new ones; abstractions and scaffolding only
  as far as the current need goes.
- Every non-trivial piece of logic gets a runnable check (an acceptance test or a `__main__` self-check);
  deleting code beats adding code.
- CI and local development share the same make targets; Clippy runs with `-D warnings` on all targets and is
  never disabled crate-wide.
- engine integration tests isolate `XDG_CONFIG_HOME`/`XDG_STATE_HOME` through a child process
  (`Command::env`, see `engine/tests/cli.rs`); when a test really must change the process environment it
  touches only its own variables and restores them within the same test.
- Leave a `ponytail:` comment on simplifications with a known ceiling (state the upgrade path).
- Credentials are read from environment variables or the machine's own stores; never write them into the
  repository, prompts, events or logs.
- Skills: the single user-level registration root is `~/.agents/skills`, covering the
  `K-Dense-AI/scientific-agent-skills` collection, `browser-use` and `find-skills`. Install companion skills
  there and read them on demand with `skill search/read`. Provenance and scope: D-34 in `docs/DECISIONS.md`.
