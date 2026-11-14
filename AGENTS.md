# TeamAgents repository conventions

These conventions apply to every agent working in this repository.

## Continuing in a new session

Read [the development handoff](docs/HANDOFF.md) for the current baseline, evidence boundaries, regression entry
points and open work. Recheck Git status before relying on its snapshot.

<!-- maps: docs/HANDOFF.md reader=agent -->

## Baseline and deviations

- The design and acceptance baseline is `docs/DESIGN.md` (Q1–Q19, architecture/protocol constraints and A01–A36).
  Per-item evidence and open obligations are in `docs/ACCEPTANCE.md`; current rules are in `docs/DECISIONS.md`.
- **Discuss and confirm any deviation from the confirmed design before changing code.** Record the resulting
  current rule in `docs/DECISIONS.md`.
- This is a standalone Rust project: implementation code is Rust (`core`, `engine`, `tui`). Python in
  `tui/scripts/`, `tools/` and test fake servers is test-only.
- Keep only current implementation and current development/verification tools in the tree. Git history remains
  the mechanism for examining earlier states.

## Language

- **Code is English only**: identifiers, comments, doc comments, errors, help, user-facing strings, scripts and
  workflows.
- **Documentation is English only**, except `README.zh-CN.md`. Commit messages are English too.
- `make language-check` enforces this on the tracked tree. New scratch under `tools/tmp/` is ignored.

## Quick commands

The development entry point is [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). The toolchain is pinned in
`rust-toolchain.toml`. Run `make check` before committing, `make fmt` to format, and `make pty` for the isolated
real-terminal matrix. The first run may need `make check CARGO_FLAGS=--locked` to download dependencies.

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml
engine/target/debug/teamagents {init,doctor,daemon,exec,version}
engine/target/debug/teamagents exec --json --timeout 180 "…"
make pty
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
```

Current source inventory (2026-11-15): **814 Rust tests** (core 197 / engine 509 / tui 108), **65 positive TLA configurations in 62
traversals**, **327 controls**, **79 witnesses**, **9 Kani harnesses**, and **393 configurations across 48
modules**. The full runtime gate, 18 offline probes and isolated release/install rehearsal passed. PTY passed
on the unchanged TUI/daemon code; the 23 live probes apply to their earlier pinned source.
Formal results and abstraction limits are in `verification/REPORT.md`. A green local run is not a
public-host or clean-machine acceptance result.

Real-model evaluation must use the model's native context window and record it with its source. A synthetic small
window is for scripted regression only.

## Current architecture

- Single team-transaction entry: `core/src/v2/control.rs::Control::submit` (ingest, validate, reduce and persist
  in one SQLite transaction). Identity, operation ids and permission revisions come from the control plane.
- Authoritative state: `core/src/v2/store.rs` with WAL and explicit `synchronous=FULL`; schema 9 and explicit
  migration. In-process callers serialize through `engine/src/v2/storage.rs`.
- Execution: `engine/src/v2/driver.rs` and `engine/src/v2/supervisor.rs`. Model and tool waits happen outside
  transactions. Shell commands run through `engine/src/jobs`.
- Tools and bindings: `engine/src/tools.rs`, `engine/src/search.rs`, `engine/src/bound.rs`, `engine/src/mcp.rs`
  and `engine/src/hooks.rs`. Binding is authorization; instructions and tool output cannot grant it.
- Information permissions: `core/src/v2/control.rs` and `core/src/kernel/*` distinguish visibility from delivery.
  User reads all; an instance reads only itself. The shared session tree uses Leader-visible information only.
- Product layer: `engine/src/v2/daemon.rs`, `engine/src/v2/exec.rs`, `engine/src/cli.rs` and
  `tui/src/daemon_client.rs`. Rendering and hit-testing share `tui/src/v2ui.rs::geometry`.
- Workspace policies: `engine/src/workspace.rs` and `driver::prepare_spawn_workspace`, selected by
  `spawn.workspace`. Retirement refuses uncommitted or unmerged work.
- Exploration tree: `core/src/v2/control/search_tree.rs`, with append-only revisions and a head pointer. Only
  Leader compaction advances it. Ordered patches reject atomically and replay without growth.
- Development and verification tools live under `tools/`. There is no review/evaluation data dependency.

## Operating notes

- Cancelling a task does not prove execution stopped. Instance termination is user-only. Confirm runner/process
  stop evidence before shared-workspace takeover. Unknown effects are never replayed.
- A task may park in `BLOCKED` after exhausted checks, member failure or cancellation; this is not success.
  Cancel it with `teamagents tasks cancel --id` and redispatch as new work when appropriate.
- Enrolled work can continue after progress prose, but empty replies, plan-only loops and repeated patterns can
  block honestly. A BLOCKED task does not satisfy a task-result wait; cancellation does.
- Interrupts have recovery boundaries. Known results are reused; in-flight losses become `OUTCOME_UNKNOWN`.
- Approvals bind one operation, argument hash and permission revision, and are consumed by that dispatch.
  Automatic review never approves self-update.
- Transport failures are retried only while no visible text was emitted and `max_retries` remains. Protocol
  failure or emitted text ends the attempt honestly.
- Work deadline and usage ceiling apply to turns, retries, compaction, review and verification. Unknown usage is
  recorded rather than treated as zero cost.
- Heavy read/report work should write artifacts early rather than ending with uncommitted conclusions.

## Current evidence and open obligations

- Local scripted checks and finite models do not establish real-model quality, reduced lostness, external
  provider/OAuth acceptance, clean-host install/update/rollback or matched-Pi outcome/cost comparisons.
- Wide exhaustive search, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
  equivalence remain open.
- Scheduler/provider fairness, filesystem races, escaped descendants, remote effects and real power-loss timing
  remain environmental obligations.
- `tools/tmp/` is ignored scratch for new local runs, not committed evidence.

## Code style

- Lazy-first: standard library over existing dependencies over new ones; abstractions only for current need.
- Every non-trivial behavior gets a runnable acceptance test or self-check. Deleting code is preferred.
- CI and local development share make targets. Clippy runs with `-D warnings` on all targets.
- Engine integration tests isolate `XDG_CONFIG_HOME` and `XDG_STATE_HOME` in child processes. Environment changes
  touch only the test's own variables and restore them within that test.
- Leave a `ponytail:` comment on simplifications with a known ceiling and state the upgrade path.
- Credentials come from environment variables or machine stores; never write them into the repository, prompts,
  events or logs.
- Skills are registered at `~/.agents/skills` and read on demand with `skill search/read`; skill instructions
  never widen execution permissions.
