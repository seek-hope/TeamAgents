# Development handoff

Updated **2026-10-22** against source **3b7ea4a1** (through D-471). This is the current entry point for a new development
session, not an authorization to implement the candidate backlog. Recheck Git status and recent commits;
this document does not assert that a remote branch or installed binary matches the working tree.

## Start here

1. Read [AGENTS.md](../AGENTS.md), then inspect Git status and recent commits.
2. Read [DESIGN.md](DESIGN.md) for Q1-Q19 and A01-A36, the current rules and newest entries in
   [DECISIONS.md](DECISIONS.md), and the baseline/current gaps in [ACCEPTANCE.md](ACCEPTANCE.md).
3. Use [DEVELOPMENT.md](DEVELOPMENT.md) for build, tests and documentation maintenance.
   [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) owns the prioritized daily-coding candidate queue,
   including the source/documentation comparison with Pi 1.3.0. Agree a bounded objective before coding.

## Current source baseline

| Source | What it contributes | Evidence boundary |
|---|---|---|
| ea43869d through a06a5150 | D-450/D-451 completion, highlighting, attachments, pending input, export/search and bounded display workers | Dated comparison/maturity receipts belong to their pinned source, not every later commit |
| c77a691a, b3898af6, 6f8a7402 | D-452 input paging, attachment cleanup/concurrency, capture process groups and display bounds | The completed D-452 independent audit is historical; it is not the latest implementation baseline |
| 11929da6 | D-455 task-scoped capability delegation, D-458 operation auto-review, D-453 updater, external editor, D-456 web tools and D-457 release freeze | Includes governed updater, external editor, DeepSeek web tools and release freeze; distinguish implemented mechanisms from real-service acceptance |
| b4e9f1f9, 90ca30d8 | D-459 stable prompt-cache prefix/telemetry and the formal report's source pin | Cache usage is not a dollar estimate; no cache warming was added |
| f78a6709 | Rust toolchain pin 1.99.1 | Read rust-toolchain.toml and the lockfiles before building |
| bdae43e2, 47546b9b | D-463/D-464 idempotent command ids and shared result digests; D-465 atomic-action windows with concurrency tests | Result bodies are derived, not authoritative; the session database stays the source of truth |
| c6dcc8a5, e1d842d1 | D-466 two-mode pluggable sandbox, D-467 AGENTS.md discovery, D-468 skill resources, D-469 multi-edit/read batch, D-470 profile/web/deferred input/cancel-vs-stop | `sandbox = none` is the explicit host mode; a configured backend still fails closed |
| 9b29c8b5, 3b7ea4a1 | D-471 acceptance checks follow the session's shell mode; CI fixtures follow `none`, transient ETXTBSY retry | `exec --check` and goal `[[checks]]` run on the host in `none` mode; the bubblewrap refusal stays covered |

D-462 added the verification gate and ledger: every counterexample configuration names the claim it must
refute (`expects:` marker, compared with TLC's output), every checked claim that no control refutes is a row in
`verification/COVERAGE.md`, and the count there is **0** — every behavioural claim now has a refuting control
or a justified `entailed-by`/`structural` status. A control that was silently refuting `TypeOK` was found and
fixed. `make verify-model-all` verifies 47 configurations and `make verify-model-counterexamples` refutes 250
controls; Kani is not installed on this machine.

D-461 implements the user's selected DD-01 option A: read-only work diagnostics and explicit recovery.
Option B (automatic model reminders) was explicitly rejected. The shared projection lives in
engine/src/v2/diagnostics.rs; checkpoint, instances, tasks and local TUI /help expose it. Tests cover query-only
reopen, real daemon restart without additional model calls, explicit input/cancellation recovery and TUI frames.

Local checks rerun on 2026-10-22 at `3b7ea4a1`: **make check passed, core 147 / engine 415 / tui 81**,
**make check-nobwrap passed** (the CI runner's bwrap-free condition) and **make pty passed** the terminal-profile
matrix (xterm/tmux/kitty profiles, auto graphics) plus both forced protocol modes. These results are local regressions, not a new independent audit, real-model
comparison, actual graphical-terminal matrix, installed-artifact acceptance or release.

The [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md) retains its immutable manifests and
receipts. The [formal report](../verification/REPORT.md) carries its own source and run boundary. Neither
is renewed by a documentation edit or a Cargo pass. All public releases/tags were removed under D-457;
source builds are the current distribution path. Publication requires separate user authorization.

## Behavior and focused regression entry points

- Pending-input pages hold at most 100 rows and 4 MiB of stored UTF-8 payload. Lengths are checked before
  loading; an oversized first input fails with its id/revision rather than being skipped or truncated.
  The limit is on payload bytes, not the entire encoded response.
- Failed attachment submission attempts immediate cleanup of that round's STAGING artifacts and files;
  boot recovery remains the crash backstop. Concurrent submissions queue on an async lock.
- Captured children lead their own process group. Failure, cancellation and timeout kill it; a completed
  capture may preserve an intentional long-lived descendant such as a daemon.
- External editing uses VISUAL, then EDITOR, and restores the draft on failure. Success changes the draft,
  never implicitly sends it. The /edit entry is recognized but not in the seven-item completion menu.
- A worker still has no ambient shell. An explicitly requested capability can derive a task-scoped child
  from a user-issued delegable parent; otherwise work waits in WAITING_CAPABILITY for a user decision.
- Automatic approval review applies only to concrete operations, not capability requests. Review failure
  leaves the approval pending. Reviewer model usage is not yet charged to the goal (D-458).
- D-459 records provider cache hit/miss/write usage and freezes the L1 masking frontier between compactions.
  No reported split means unknown cache behavior, not a measured miss.

Useful checks, all included in the normal suites:

| Area | Regression location |
|---|---|
| Input paging/edit/cancel/restart and concurrent attachments | engine/tests/cases/inputs.rs, engine/src/v2/attachments_tests.rs |
| Process-group capture and draft/attachment ownership | tui/src/subprocess.rs, tui/tests/attachments.rs |
| External editor success/failure | tui/src/editor.rs |
| Delegable capabilities and operation-only review | core/src/v2/control.rs tests, engine/tests/v2_driver.rs, engine/src/v2/reviewer.rs |
| Cache usage, stable prefixes and provider request bodies | core/src/kernel/mod.rs, engine/tests/providers_fake.rs, engine/src/v2/exec.rs |
| Governed update transactions | engine/src/update.rs, engine/tests/update.rs |

## Verification and environment

There is no workspace-root Cargo manifest. Use make or explicit crate manifests. Run leak-guard gates
serially with live probes; their process/scratch censuses can otherwise interfere.

```bash
git status --short --branch
git log -5 --oneline
make check
make pty
```

Build output requires writable space; use CARGO_TARGET_DIR when checking a read-only source mount.
Keep the actual installed Rust/Cargo homes available rather than hiding them with an arbitrary HOME/PATH
change. An unavailable Docker daemon can produce a documented skip/refusal path; a green gate does not
establish a working live Docker backend or external provider.

The D-451 verifier checks exact source hashes. Run its offline verification at a06a5150, using a detached
worktree or temporary Git archive. It is expected to reject later source. Never rewrite a frozen manifest
to make current code appear to have passed an older run; new acceptance needs new receipts.

## Files and state to preserve

- Tracked source, docs, frozen evaluations and immutable receipts are the portable handoff. Temporary
  check logs and ignored agent journals are not required to resume development.
- The root session.sqlite is tracked project data, not disposable scratch. Do not delete or migrate it
  without understanding its provenance and the requested operation.
- Ignored agent state, logs, TLC states and build outputs may exist beside tracked files. Do not use broad
  Git clean, database deletion or process-kill commands to prepare a conversation.
- Preserve the user's configs, sessions and installation, including the default state under
  ~/.local/state/teamagents/v2. Tests use isolated HOME/XDG/state/workspace directories.
- Credentials remain in environment variables or machine stores, never in docs, prompts, events or logs.

## Next-cycle constraints

Prioritize the agreed items from the comparison queue rather than copying its whole backlog into a goal.
DD-01 option A is implemented; do not add automatic reminders under that authorization. The next candidates
are real development/skill environments and a paired coding evaluation; instruction discovery, tool composition, session controls and complete accounting follow.
External editing, capability delegation and cache telemetry are already implemented, with remaining
coverage limits stated separately. Paid trials and releases require concrete scope and authorization.

Keep the Rust control plane, single SQLite authority, independent daemon and minimal Leader TUI with CLI
management. Interrupt cancels a model request, not tools; exit detaches; deletion stays identity-bound;
approvals do not steal the composer; unknown effects are not replayed. Automatic cross-session injection,
distributed/multi-tenant execution and a marketplace remain excluded. Design deviations need confirmation.

The user's terminal is Kitty with zsh. Keep graphical-terminal evidence separate from PTY protocol tests
and preserve owned-image cleanup. Code/docs are English except README.zh-CN.md and frozen evidence.
Generated references and their inventories must remain synchronized; never weaken a gate to hide drift.
