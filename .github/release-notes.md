> Current source validation: core 197 / engine 509 / tui 108, plus 65 positive TLA configurations in 62
> traversals, 327 controls, 79 witnesses and 9 Kani harnesses. The latest `make check` passed. This document describes
> current source, not an earlier release archive.

## Current source highlights

- One authoritative SQLite database per session with atomic work, grants, approvals, receipts and events.
  Recovery reuses verified results and records unknown outcomes without blind replay.
- Persistent work across turns, compaction and restart, with bounded continuation, required checks, budgets,
  deadlines and internal admission of subsequent work.
- One session-level exploration tree with revisioned nodes, source references, focus, Leader-only writes and
  ordered atomic patches. `/tree` and `teamagents search_tree [--json]` expose it; member-private history is not
  included.
- One daemon per session. TUI and `teamagents exec` share its Unix-socket JSON protocol and resume from the event
  watermark.
- Governed teams through `spawn`, `delegate`, `send` and `wait`, with shared, isolated and Git-worktree
  workspaces and task-scoped capability delegation.
- Chat Completions, DeepSeek, Responses and Anthropic profiles, native context windows and configurable
  reasoning effort.
- Host Shell, acceptance commands and local MCP with explicit capability checks. Ordinary Shell requires no
  per-call approval; self-update approval remains. Retired sandbox settings are rejected.
- File tools, persistent Shell state, DeepSeek/Exa search, DeepSeek fetch, Skills, stdio/HTTP MCP resources and
  restricted JavaScript codemode. Hooks can report events or veto native tool calls.
- Current executable development and verification tools under `tools/`.

## Current evidence

The full local gate passed **814 Rust tests** (core 197, engine 509, TUI 108). Formal material contains **393
configurations across 48 modules**: **65 positive configurations in 62 traversals**, **327 negative controls**,
**79 normal-path witnesses** and **9 Kani harnesses**. Search-tree coverage includes ordered pair transitions,
23 controls covering every non-`TypeOK` tree claim and 11 witnesses. Witnesses demonstrate reachability only.

Earlier pinned builds passed five PTY terminal profiles and both forced graphics modes,
`make probe-offline` (18/18) and `make probe-models` (23/23). These are local scripted, finite-model and targeted real-model checks, not
blanket multi-host or full-benchmark acceptance.

## Current boundaries

Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
equivalence remain open. Earlier pinned builds passed public SSH/screen reconnect and isolated installation,
upgrade and rollback on an existing Debian 12 host. External OAuth services, freshly provisioned hosts,
multi-distribution matrices and large-scale long-horizon comparisons remain open. No installed runtime replacement or public release is
implied by this document.
