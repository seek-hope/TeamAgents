> Current source validation: core 198 / engine 522 / tui 117, plus 63 positive TLA configurations in 60
> traversals, 323 controls, 79 witnesses and 9 Kani harnesses. The latest `make check` passed. This document describes
> current source, not an earlier release archive.

## Current source highlights

- One authoritative SQLite database per session with atomic work, grants, approvals, receipts and events.
  Recovery reuses verified results and records unknown outcomes without blind replay.
- Persistent work across turns, compaction and restart, with bounded continuation, required checks, budgets,
  deadlines and internal admission of subsequent work.
- One session-level exploration tree with revisioned nodes, source references, focus, Leader-only writes and
  ordered atomic patches. `/tree` and `teamagents search_tree [--json]` expose it; member-private history is not
  included.
- Herdr-inspired team workspace with member state, saved blockers, tasks, activity and separate member history.
  Responsive panes preserve drafts and approval focus; all message and profile controls still target Leader.
- Named profiles resolve before model preparation and recovery, preventing catalog aliases from reaching the
  API and applying the selected effort to the first request. All fifteen local slash commands are discoverable.
- Fetch verifies public hostnames independently when local DNS returns fake-IP routing addresses, while
  retaining private-destination refusal. Returned text respects its UTF-8 byte limit.
- One daemon per session. TUI and `teamagents exec` share its Unix-socket JSON protocol and resume from the event
  watermark.
- Governed teams through `spawn`, `delegate`, `send` and `wait`, with shared, isolated and Git-worktree
  workspaces and task-scoped capability delegation.
- Chat Completions, DeepSeek, Responses and Anthropic profiles, native context windows and configurable
  reasoning effort.
- Host Shell, acceptance commands and local MCP with explicit capability checks. Ordinary Shell requires no
  per-call approval; self-update approval remains. Retired sandbox settings are rejected.
- File tools, persistent Shell state, DeepSeek/Exa search, DeepSeek fetch, Skills, stdio/HTTP MCP resources and
  restricted JavaScript codemode. Batch file edits reject malformed supplied hash preconditions without
  modifying files. Hooks can report events or veto native tool calls.
- File-open flags follow the target ABI; the file/tool regression subset runs under GNU
  user emulation in a dedicated CI job. This does not add an ARM release package.
- Session lifecycle actions preserve listening or uncertain endpoints even when their greetings fail, and
  explicit stop propagates failure. Legacy protocol shutdown remains supported.
- Current executable development and verification tools under `tools/`.

## Current evidence

The full local gate passed **837 Rust tests** (core 198, engine 522, TUI 117). Formal material contains **387
configurations across 47 modules**: **63 positive configurations in 60 traversals**, **323 negative controls**,
**79 normal-path witnesses** and **9 Kani harnesses**. Search-tree coverage includes ordered pair transitions,
23 controls covering every non-`TypeOK` tree claim and 11 witnesses. Witnesses demonstrate reachability only.

The current x86_64 Linux musl runtime passed release/install rehearsal, source/installed PTY matrices,
18 offline probes and scripted profile/retry/tool/fetch checks. A separate real Fetch API check passed.
The four delivery CI jobs passed; exact source/package bindings are in `docs/ACCEPTANCE.md`.
AArch64 coverage is 26 tool regressions under GNU user emulation, not native ARM or an ARM release package.

## Current boundaries

Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
equivalence remain open. Real-model outcome quality, external OAuth, public hosts/graphical terminals,
clean-host install/update/rollback, schema transitions, native ARM, broader distributions and matched
long-horizon/priced-cost comparisons need separate acceptance. No installation replacement or public release
is implied by this document.
