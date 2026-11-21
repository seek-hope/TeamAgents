## Current source highlights

- One authoritative SQLite database per session with atomic work, grants, approvals, receipts and events.
  Recovery reuses verified results and records unknown outcomes without blind replay.
- Persistent work across turns, compaction and restart, with bounded continuation, required checks, budgets,
  deadlines and internal admission of subsequent work.
- One session-level exploration tree with revisioned nodes, source references, focus, Leader-only writes and
  ordered atomic patches. `/tree` and `teamagents search_tree [--json]` expose it; member-private history is not
  included.
- Herdr-inspired team workspace with member state, saved blockers, tasks, activity and separate member history.
  Responsive panes preserve drafts and approval focus. Session controls select the Leader model and the shared Worker policy.
- Named profiles resolve before model preparation and recovery, preventing catalog aliases from reaching the
  API and applying the selected effort to the first request. Slash commands are discoverable in the TUI.
- Fetch verifies public hostnames independently when local DNS returns fake-IP routing addresses, while
  retaining private-destination refusal. Returned text respects its UTF-8 byte limit.
- One daemon per session. TUI and `teamagents exec` share its Unix-socket JSON protocol and resume from the event
  watermark.
- Governed teams through `spawn`, `delegate`, `send` and `wait`, with shared, isolated and Git-worktree
  workspaces and task-scoped capability delegation.
- Chat Completions, DeepSeek, Responses, Anthropic and Gemini profiles, native context windows and configurable
  reasoning effort.
- Host Shell, acceptance commands and local MCP with explicit capability checks. Ordinary Shell requires no
  per-call approval; self-update approval remains. Retired sandbox settings are rejected.
- File tools, persistent Shell state, DeepSeek/Exa search and fetch, Skills, stdio/HTTP MCP resources and
  restricted JavaScript codemode. Batch file edits reject malformed supplied hash preconditions without
  modifying files. Hooks can report events or veto native tool calls.
- File-open flags follow the target ABI; the file/tool regression subset runs under GNU
  user emulation in a dedicated CI job. This does not add an ARM release package.
- Session lifecycle actions preserve listening or uncertain endpoints even when their greetings fail, and
  explicit stop propagates failure. Legacy protocol shutdown remains supported.
- Current executable development and verification tools under `tools/`.

## Current evidence

The source contains **893 Rust tests** (core 210 / engine 561 / tui 122). Formal material contains **387
configurations across 47 modules**: **63 positive configurations in 60 traversals**, **323 negative controls**,
**79 normal-path witnesses** and **9 Kani harnesses**. Search-tree coverage includes ordered pair transitions,
23 controls covering every non-`TypeOK` tree claim and 11 witnesses. Witnesses demonstrate reachability only.

`make release-rehearsal` exercises the x86_64 Linux musl archive. `make pty` exercises the TUI in a real terminal.
`make probe-offline` runs the local product probes. `make test-aarch64` covers 26 tool regressions under GNU user emulation.

## Current boundaries

Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
equivalence remain open. Real-model outcome quality, external OAuth, public hosts/graphical terminals,
clean-host install/update/rollback, schema transitions, native ARM, broader distributions and matched
long-horizon/priced-cost comparisons need separate acceptance. No installation replacement or public release
is implied by this document.
