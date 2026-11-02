# Development handoff

Updated **2026-11-03**. **D-494** repairs provider/runner ingestion and supervision, durable start acceptance,
storage-backpressure recovery, idle context loading and TUI rendering/frame deadlines. The material source
and installed native binaries are pinned to `0027ffe2ffbbba57acf852f59bf8904dce5b0ac3`. Both binaries were
privately backed up and atomically replaced; installed init/schema-7/idle/shutdown and conversation/graphics
PTY checks passed without model requests. No user daemon was started and the user config is unchanged.
D-491's shortcuts, D-490's colors and D-487's control/data contracts remain.
Use [ACCEPTANCE.md](ACCEPTANCE.md) for gate evidence and [DECISIONS.md](DECISIONS.md) for the confirmed
contract. Recheck Git status: ten documentation files had preexisting changes when D-494 began and are
preserved outside its scoped commits.
Earlier delivery: both binaries were replaced from D-492 after private backups and isolated smoke.
At the user's subsequent request, old backup binaries and the old-source probe build were removed while
read-only session backups were retained, and the existing user daemon was stopped normally with committed
state kept. No runner process remained. A later client starts a fresh daemon. D-493 subsequently replaced the
public v0.2.0 tag and release from `bee64381`; its workflow and published-install check passed. The archive
SHA-256 is `bf527c449f098060e76b0fc7055643bbef448ba4aa9734a063f2ea4dc28f3934`.

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

The TUI remains one Leader conversation, now adapted from Grok Build's single-agent presentation under D-489:
elevated user prompts, operation-derived tool groups, quiet model chrome and a conditional live activity row.
D-490 replaces the chrome palette with Codex's terminal-default/ANSI roles and uses its default dark syntax
colors; terminal probing and automatic light/dark theme selection are not imported. D-491 keeps newline
on Shift+Enter, retires Ctrl+J and places Ctrl+S sessions in the ready/draft footer.
D-492 adds dependency-advisory CI, bounded OAuth/MCP/update inputs and locked atomic credential writes.
Its [dated review](../review/security-2026-10-29/REPORT.md) records fixes and remaining maintenance notices.
`@PATH` plus Tab stages file references through the existing confined attachment flow. Inline questions,
conditional approvals, complete Markdown, readable Shell output, editing, shortcuts, media placement and
streaming/saved rendering retain their implementations. Default search is DeepSeek + Exa; fetch remains
DeepSeek-only. CodeGraph stays optional MCP + codemode, with existing indexes, shared-daemon locking and
explicit initialization.

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
| Provider framing and bounded bodies | `engine/tests/cases/provider_robustness.rs`; `engine/src/providers/stream.rs` unit tests |
| Runner persistence, supervision, cancellation and retirement | `engine/tests/cases/jobs_robustness.rs`; included by `jobs_runner` |
| Storage saturation and idle context reads | `engine/tests/cases/supervisor_robustness.rs`; included by `v2_supervisor` |
| Formal transition contracts | `verification/tla/V2Work.tla`, `verification/tla/V2JobSupervision.tla`, `verification/tla/V2Discovery.tla`; control/wait retain their legacy scheduler scope |

Run `make check` before committing; `make fmt` also formats the two new included test files. Use the formal
and PTY targets documented in DEVELOPMENT. Do not replace a failed behavioral check with a weaker assertion.

## Evidence and remaining work

Current local regression belongs in ACCEPTANCE; formal results and abstraction limits belong in
[verification/REPORT.md](../verification/REPORT.md). The [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md)
and D-452 audit keep their original source pins. Passing later tests does not renew them.

The [D-494 review](../review/optimization-2026-11-03/REPORT.md) records old-code failures and a synthetic debug\nredraw fixture: at 201,599 wrapped rows, median frame time changes from 77.206 to 1.812 ms and allocations\nfrom 49,052,488 to 23,210 bytes/frame. Full-history hashing remains linear. The formal catalogue has 41\nmodules, 50 positive configurations, 273 negative controls and one uncompleted wide configuration; seven\nKani arithmetic harnesses pass. Catalogue completeness is not Rust/SQLite refinement or full-system proof.\n\nThe [goal-autonomy source study](../review/goal-autonomy-2026-10-28/REPORT.md) informed D-487; upstream source
inspection is not an executed comparison. Local scripted providers and process tests do not establish real-model
success, comparative cost or reduced user intervention. Reviewer accounting, broader provider/terminal
acceptance and matched Pi trials remain separate candidates. No paid trial or public release is part of D-487.
