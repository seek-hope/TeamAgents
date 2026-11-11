# Development handoff

D-502 closes reproduced automatic-review parsing, authorization-context, control-evidence and known-usage gaps.
Its dated adversarial report distinguishes those implementation fixes from still-open live-model judgement
quality. Actual tmux and isolated loopback-SSH PTY probes pass; neither is a real-provider or public-host matrix.

D-501 upgrades `rusqlite`/`libsqlite3-sys` past RUSTSEC-2026-0459 while preserving the schema-8/WAL contract.
Core and engine suites and all dependency advisory/source checks pass.

D-500 adds lazy input revisions, reviewer budget/usage recovery, real Cargo/Python/Node Skill workflows and
MCP resource list/read. It preserves schema, authority and cancellation contracts; the remaining live-service,
matched-Pi, general-SDK and full-proof obligations stay open.

D-498/D-499 are the previous committed baseline (`53547c3b`): they removed ten unused persistence mirror types
and consolidated duplicate handoff/development guidance. D-500 (`bdfc2e27`) and D-501 (`cd6125f9`) then added
the accounting/workflow/resource fixes and SQLite security upgrade without changing the schema-8 contract.

The goal entity and its public vocabulary are removed in favor of work scopes, with schema 8 migration
preserving old budget, acceptance, ownership and event history. The full 738-test repository gate, 58 positive
configurations/55 traversals, 298 controls, 65 witnesses, nine Kani harnesses and the complete PTY matrix pass.
The first positive replay timed out at 2400 seconds and is not counted; the complete replay has actual exit 0
in 2070.4 seconds. These pre-commit local receipts are pinned by the material commit; they are not a
canonical-allocation proof. Runtime/installed binaries and the public release remain unchanged.

Earlier baselines are kept as history rather than repeated as current work. D-496 formal material is pinned at
`61feaf5fe0682632a1203607c21e0df39c86f73f`; D-494 runtime/install material is pinned at
`0027ffe2ffbbba57acf852f59bf8904dce5b0ac3`; the public v0.2.0 archive remains D-493. See the
[formal report](../verification/REPORT.md), [formal search report](../review/formal-optimization-2026-11-03/REPORT.md)
and [robustness review](../review/optimization-2026-11-03/REPORT.md) for their exact evidence and limits.
Current acceptance evidence belongs in [ACCEPTANCE.md](ACCEPTANCE.md), not in copied status paragraphs here.

## Start here

1. Read [AGENTS.md](../AGENTS.md), then inspect `git status --short --branch` and `git log -5 --oneline`.
2. Read the current rules in [DESIGN.md](DESIGN.md) and the current-rule table/latest entries in DECISIONS.
   Older decision entries are dated evidence; a superseded rule is not a current instruction.
3. Use [DEVELOPMENT.md](DEVELOPMENT.md) for the shared local/CI gates and generated references.
   [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) is the only candidate-work queue. Implementing a candidate,
   a paid trial and a public release are separate scopes.

## Current behavior

The shared behavior prompt prioritizes user instructions and intent, initiative, free exploration and excellent
results within the authorized scope. Routine implementation choices need no repeated confirmation. A work
carries the whole admitted work across turns, compaction and restart. Questions, event waits, blocking, stops
and completion are distinct; a progress reply is not completion. Checkpoints are model claims, not authority.

After terminal work, information-only replies create no work; actionable requests are admitted internally.
Admission retains the original input, policy, deadline and known/unknown usage. New input cannot replenish an
active budget or resume PARKED work. Stale preparation, responses and completion cannot take over a new
assignment. Required checks and scoped evidence still gate success. Retention keeps pending/live input
envelopes, including acceptance metadata, until work closes or the instance terminates.

Schema **v8** uses work scopes and work IDs throughout. Its explicit v7 migration preserves the old scope
records, references and events without auto-enrolling old idle work into the continuation policy. Forking keeps
decisions, pending costs and user stops but not execution; it requires fresh execution evidence and refuses
tracked execution without stop evidence. Export labels checkpoint provenance.
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

Run `make check` before committing; `make fmt` formats every crate, including test files loaded with `include!`.
Use the formal and PTY targets documented in DEVELOPMENT. Do not replace a failed behavioral check with a weaker assertion.

## Evidence and remaining work

Current local regression belongs in ACCEPTANCE; formal results and abstraction limits belong in
[verification/REPORT.md](../verification/REPORT.md). The [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md)
and D-452 audit keep their original source pins. Passing later tests does not renew them.

The D-494 [review](../review/optimization-2026-11-03/REPORT.md) records old-code failures and a synthetic debug
redraw fixture: at 201,599 wrapped rows, median frame time changes from 77.206 to 1.812 ms and allocations
from 49,052,488 to 23,210 bytes/frame. Full-history hashing remains linear. Its historical formal catalogue had 41
modules, 50 positive configurations, 273 negative controls and one uncompleted wide configuration; seven
Kani arithmetic harnesses passed. D-495's counts remain in its historical report. Catalogue completeness is not
Rust/SQLite refinement or full-system proof.

The [work-autonomy source study](../review/goal-autonomy-2026-10-28/REPORT.md) informed D-487; upstream source
inspection is not an executed comparison. Local scripted providers and process tests do not establish real-model
success, comparative cost or reduced user intervention. Reviewer judgement quality, broader provider/terminal
acceptance and matched Pi trials remain separate candidates. No paid trial or public release is part of D-487.
