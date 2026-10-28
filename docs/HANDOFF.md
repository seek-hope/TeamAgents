# Development handoff

Updated **2026-10-26**. The runtime baseline is **D-480 (`7ae6bd5b`)**; **D-481** refreshes TUI presentation
without changing session, protocol or execution semantics. It uses brighter secondary text, quiet rounded
panels and textual selection markers across the conversation, completions, queue and session pages. No new
TUI features or interactions are added. Always recheck Git status and recent commits. This file records a
starting point, not the state of the remote branch, an installed binary or permission to implement the
candidate backlog.

## Start here

1. Read [AGENTS.md](../AGENTS.md), then run `git status --short --branch` and `git log -5 --oneline`.
2. Read [DESIGN.md](DESIGN.md) for Q1–Q19 and A01–A36, the current-rule table and latest entries in
   [DECISIONS.md](DECISIONS.md), and the evidence boundaries in [ACCEPTANCE.md](ACCEPTANCE.md).
3. Use [DEVELOPMENT.md](DEVELOPMENT.md) for builds, tests and documentation maintenance.
   [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) is the only candidate-work queue. Agree on a bounded
   objective before changing behavior.

## Current baseline

D-480 removed the OS sandbox backends. Shell, acceptance commands and local MCP services run on the host;
ordinary Shell needs no per-command approval. Task capabilities, operation records, recovery and self-update
approval remain. Retired sandbox configuration is rejected rather than silently downgraded. Existing sessions,
VMs, packages and network policies are not removed by the migration.

The last complete D-480 verification passed `make check` with **core 147 / engine 415 / tui 87**, plus the full
`make pty` matrix. Local release binaries passed host discovery, the installed TUI smoke and a daemon-stop /
runner-recovery check in which the command ran once and its saved result was imported after restart. These are
local regression results, not an independent audit, real-provider trial or public-release replacement.

D-479 added host tool discovery, boot-scoped environment/config/source observations and input-delivery
receipts. It does not hot-reload configuration or prove that a discovered toolchain works. D-461 implements
read-only work diagnosis and explicit recovery instructions; automatic model reminders are not approved.

D-481 passed `make check` with **core 147 / engine 415 / tui 91** and the complete `make pty` matrix. Four
focused visual tests cover quiet semantic roles, readable message bodies and shared rounded panels/selection.
Local release-profile binaries were installed after backup, and the installed TUI passed the xterm-256color
conversation PTY without changing user configuration or sessions. These are presentation regressions, not
evidence of changed runtime behavior or a new release.

Older evidence stays pinned to its own source:

- [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md): immutable manifests and local workflow
  receipts. Its verifier is expected to reject later source.
- [Formal verification report](../verification/REPORT.md): recorded TLA+/Kani scope and unproven areas. Kani
  was unavailable on this machine; a later Cargo pass does not renew the proofs.
- v0.2.0 is the first public release. Current source may differ from that archive. Future publication needs
  separate authorization and the repository release gate.

## Contracts to preserve

- `core/src/v2/control.rs::Control::submit` is the single team-transaction path. The control plane assigns
  identity, operation ids and permission revisions; models and clients do not supply them.
- `core/src/v2/store.rs` owns one SQLite database per session, with WAL and `synchronous=FULL`.
  `engine/src/v2/storage.rs` serializes in-process writers.
- Model and tool waits happen outside database transactions. Recovery uses persisted locations and runner
  journals; unknown effects are not replayed.
- Interrupting a model request, cancelling a task, terminating an instance and closing the TUI are different
  operations. Task cancellation alone does not stop a running process group.
- Workers have no ambient Shell. Capability requests and operation approvals are distinct. A `BLOCKED` task
  does not satisfy a task wait; `CANCELLED` does.
- Project trust does not let a repository supply hooks, checks, retention, limits or update policy.
- Host Shell is not workspace confinement. File-tool path checks, controlled environment inheritance and
  credential filtering remain separate mechanisms.
- A conversation branch is not a workspace rollback. Logs and exports are ordinary same-user files, not
  tamper-proof records or complete external-side-effect backups.

## Focused regression entry points

All rows below are included in the normal suites unless noted otherwise.

| Area | Location |
|---|---|
| Input paging, editing, cancellation, restart and attachments | `engine/tests/cases/inputs.rs`, `engine/src/v2/attachments_tests.rs` |
| Process groups, drafts and attachment ownership | `tui/src/subprocess.rs`, `tui/tests/attachments.rs` |
| External editor | `tui/src/editor.rs` |
| Capability delegation and operation review | `core/src/v2/control.rs`, `engine/tests/v2_driver.rs`, `engine/src/v2/reviewer.rs` |
| Provider cache fields and stable prompt prefixes | `core/src/kernel/mod.rs`, `engine/tests/providers_fake.rs`, `engine/src/v2/exec.rs` |
| Governed self-update | `engine/src/update.rs`, `engine/tests/update.rs` |
| Host execution, legacy config and saved-job recovery | `engine/src/tools.rs`, `engine/tests/cli.rs`, `engine/tests/v2_mcp.rs`, `engine/src/v2/driver.rs` |
| Environment/config/source observations and input receipts | `engine/src/environment.rs`, `engine/tests/v2_daemon.rs`, `tui/tests/minimal.rs` |
| Work diagnosis and recovery hints | `engine/src/v2/diagnostics.rs`, `engine/tests/v2_daemon.rs`, `tui/tests/minimal.rs` |

Useful limits to check when editing related code:

- Pending-input pages hold at most 100 rows and 4 MiB of stored UTF-8 payload. Oversized first inputs fail
  explicitly rather than being skipped or truncated.
- Failed attachment submission cleans that round's staging artifacts; boot recovery handles crashes.
  Concurrent submissions use an async lock.
- Captured children lead a process group. Cancellation and timeout kill that group; a deliberately detached
  service can outlive the command and must be managed explicitly.
- External editing uses `VISUAL`, then `EDITOR`, restores drafts on failure and never sends automatically.
- Reviewer usage is not yet included in goal accounting. Provider cache fields describe reported usage, not
  monetary cost.

## Verification commands

There is no root Cargo workspace. Use `make` or each crate's manifest. Run leak-guarded suites serially with
live probes because their process and scratch-directory inventories can interfere.

```bash
make check
make pty
make verify-model-all
make verify-kani       # requires the Kani toolchain
```

Build output needs writable space; use `CARGO_TARGET_DIR` for a read-only source mount. Keep the actual Rust
and Cargo homes available. Shell and local MCP tests run on the host and need no external sandbox package.
The PTY matrix checks terminal protocols, not every graphical terminal or OS.

## Files and state to preserve

- Tracked source, docs, frozen evaluations and immutable receipts are the portable handoff. Temporary logs
  and ignored agent journals are not required to resume development.
- The root `session.sqlite` is tracked project data, not disposable scratch. Do not delete or migrate it
  without understanding its provenance and the requested operation.
- Do not use broad `git clean`, database deletion or process-pattern kills to prepare a workspace.
- Preserve user configs, sessions and installations, including `~/.local/state/teamagents/v2`. Tests use
  isolated HOME/XDG/state/workspace directories.
- Keep credentials in environment variables or machine stores, never in repository files, prompts or logs.

## Next-cycle constraints

The current task is documentation and maintenance cleanup, not new runtime behavior. Later candidates are
actual development/Skill workflows (DD-02) and wider operational evidence. The matched Pi trial is postponed;
monetary accounting is outside the current scope. Do not add automatic reminders under D-461.

Keep the Rust control plane, per-session SQLite authority, independent daemon and minimal Leader TUI.
Automatic cross-session injection, distributed/multi-tenant execution and a marketplace remain excluded.
Generated references and inventories must stay synchronized; never weaken a gate to hide drift. Code and docs
are English except `README.zh-CN.md` and frozen evaluation evidence.
