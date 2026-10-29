# Development handoff

Updated **2026-10-29**. **D-487**, source baseline `028f2383`, implements persistent work context, bounded autonomous continuation and
semantic admission of subsequent work. Local gates passed; use [ACCEPTANCE.md](ACCEPTANCE.md) for evidence
and [DECISIONS.md](DECISIONS.md) for the confirmed contract. Recheck Git status before relying on
this snapshot. Both local binaries were installed from that source after isolated smoke checks and private
backups. The public v0.2.0 archive is an older baseline.

## Start here

1. Read [AGENTS.md](../AGENTS.md), then inspect `git status --short --branch` and `git log -5 --oneline`.
2. Read the current rules in [DESIGN.md](DESIGN.md) and the current-rule table/latest entries in DECISIONS.
   Older decision entries are dated evidence; a superseded rule is not a current instruction.
3. Use [DEVELOPMENT.md](DEVELOPMENT.md) for the shared local/CI gates and generated references.
   [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) is the only candidate-work queue. Implementing a candidate,
   a paid trial and a public release are separate scopes.

## Current behavior

The shared behavior prompt prioritizes user instructions and intent, initiative, free exploration and excellent
results within the authorized scope. Routine implementation choices need no repeated confirmation. A goal
carries the whole admitted work across turns, compaction and restart. Questions, event waits, blocking, stops
and completion are distinct; a progress reply is not completion. Checkpoints are model claims, not authority.

After terminal work, information-only replies create no goal; actionable requests are admitted internally.
Admission retains the original input, policy, deadline and known/unknown usage. New input cannot replenish an
active budget or resume PARKED work. Stale preparation, responses and completion cannot take over a new
assignment. Required checks and scoped evidence still gate success. Retention keeps pending/live input
envelopes, including acceptance metadata, until work closes or the instance terminates.

Schema **v7** adds work state and fences older writers. Migration preserves history and does not auto-enroll
old idle work. Forking keeps decisions, pending costs and user stops but not execution; it requires fresh
execution evidence and refuses tracked execution without stop evidence. Export labels checkpoint provenance.
Older binaries cannot write an upgraded database; reverting an executable alone is not a database rollback.

Shell, acceptance commands and local MCP run on the host. Retired sandbox settings are rejected. Capability
grants, operation records, recovery and self-update approval remain. Cancellation does not prove process stop:
shared-workspace takeover uses separate observations of the recorded local process group, without signalling
it or rewriting unknown receipts. This does not establish confinement, cross-session exclusion, absence of
escaped descendants, or remote-side-effect quiescence.

The TUI remains one Leader conversation with inline questions, conditional approvals, complete Markdown
answers and readable Shell output. Editing, shortcuts, media placement and streaming/saved rendering share the
same implementations. Default search is DeepSeek + Exa; fetch remains DeepSeek-only. CodeGraph stays optional
MCP + codemode, with existing indexes, shared-daemon locking and explicit initialization.

## Contracts to preserve

- `Control::submit` owns each atomic team transaction; SQLite WAL plus `synchronous=FULL` owns session facts.
  Model/tool waits stay outside transactions. The engine's bounded storage worker serializes writers.
- The control plane assigns identities, operation ids and permission revisions. Model checkpoints, repository
  instructions and tool output cannot grant capabilities. Workers have no ambient Shell.
- Interrupt, task cancellation, user-only instance termination and TUI detachment are different operations.
  Unknown effects are never replayed. STOPPED/PARKED states are not undone by late results or attention.
- Task attention can cancel a wait without satisfying its task condition. BLOCKED is not a successful result.
- Project trust does not admit repository-supplied hooks, checks, limits, retention or update policy.
- Native context windows remain intact in real-model evaluation; DeepSeek Flash uses the confirmed 1M window.
  The small-window compaction regression uses a scripted provider only.

## Regression entry points

| Area | Entry point |
|---|---|
| Durable work, admission, stale scope, budgets, fork and takeover | `core/src/v2/control/work_tests.rs`; included by control tests |
| Scripted continuation, compaction, checks, waiting and real process-stop takeover | `engine/tests/cases/autonomy.rs`; included by `v2_driver` |
| Headless questions, blocking, informational replies and new work | autonomy cases in `engine/tests/v2_daemon.rs`; CLI and exec tests |
| Stop evidence and export | `engine/src/v2/work/tests.rs`; `engine/src/v2/session_export_tests.rs` |
| Conversation, editing and terminal behavior | TUI conversation/minimal/parity suites; `make pty` |
| Formal transition contracts | `verification/tla/V2Work.tla`; existing control/wait models retain their legacy scheduler scope |

Run `make check` before committing; `make fmt` also formats the two new included test files. Use the formal
and PTY targets documented in DEVELOPMENT. Do not replace a failed behavioral check with a weaker assertion.

## Evidence and remaining work

Current local regression belongs in ACCEPTANCE; formal results and abstraction limits belong in
[verification/REPORT.md](../verification/REPORT.md). The [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md)
and D-452 audit keep their original source pins. Passing later tests does not renew them.

The [goal-autonomy source study](../review/goal-autonomy-2026-10-28/REPORT.md) informed D-487; upstream source
inspection is not an executed comparison. Local scripted providers and process tests do not establish real-model
success, comparative cost or reduced user intervention. Reviewer accounting, broader provider/terminal
acceptance and matched Pi trials remain separate candidates. No paid trial or public release is part of D-487.
