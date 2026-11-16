> Current source validation: core 198 / engine 522 / tui 117, plus 65 positive TLA configurations in 62
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
- Target-ABI file-open flags fix AArch64 EINVAL failures; the file/tool regression subset runs under GNU
  user emulation in a dedicated CI job. This does not add an ARM release package.
- Session lifecycle actions preserve listening or uncertain endpoints even when their greetings fail, and
  explicit stop propagates failure. Legacy protocol shutdown remains supported.
- Current executable development and verification tools under `tools/`.

## Current evidence

The full local gate passed **837 Rust tests** (core 198, engine 522, TUI 117). Formal material contains **393
configurations across 48 modules**: **65 positive configurations in 62 traversals**, **327 negative controls**,
**79 normal-path witnesses** and **9 Kani harnesses**. Search-tree coverage includes ordered pair transitions,
23 controls covering every non-`TypeOK` tree claim and 11 witnesses. Witnesses demonstrate reachability only.

The `a6b5555a` package passed release/install rehearsal, an installed batch-edit refusal/repair workflow
with a scripted loopback provider, and a session-prune preservation check. That runtime also passed the
current PTY matrix and all 18 offline probes. D-521's AArch64 GNU user-emulation checks remain bound to
`675af52e`; they are not native ARM acceptance. The earlier `0eb4ee98` runtime passed `make probe-offline` (18/18), release rehearsal and fresh Debian 12/Ubuntu 22.04.5
KVM checks for ordinary-user installation, malformed-argument rejection, same-schema upgrade/rollback and
failed-startup recovery. Those guests used scripted loopback providers. Earlier pinned builds passed five PTY
terminal profiles, both forced graphics modes and `make probe-models` (23/23). These are scoped checks.

## Current boundaries

Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
equivalence remain open. Earlier pinned builds passed public SSH/screen reconnect and isolated installation,
upgrade and rollback on an existing Debian 12 host. External OAuth services, physical hosts, broader distribution
and architecture matrices, schema-transition acceptance and large-scale long-horizon comparisons remain open.
No normal host installation replacement or public release is implied by this document.
