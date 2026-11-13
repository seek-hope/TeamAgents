# Current implementation decisions

This file records only the rules currently in force and the latest confirmed change. It is not a decision history.
The design baseline is [DESIGN.md](DESIGN.md), acceptance evidence is in [ACCEPTANCE.md](ACCEPTANCE.md), and
verification boundaries are in [verification/REPORT.md](../verification/REPORT.md).

## Earlier rules that still apply

This table records only current contracts and their identifiers. It contains no earlier behavior, run result or
evaluation evidence.

| Topic | Current contract | Origin |
|---|---|---|
| Custom providers | Compatible services are configured through model profiles | D-40 |
| Daemon identity | A client learns session/workspace identity from the running daemon | D-41, D-57 |
| Headless outcomes | A run reports its own outcome and does not infer it from prior events | D-49, D-72 |
| No ambient Shell | Spawned instances need explicit scoped grants | D-61 |
| MCP | Bound services use the shared authority, budget, cancellation and receipt paths | D-74 |
| Retired external profile | The retired external execution profile is refused | D-75 |
| Web bindings | Search/fetch tools exist only when configured and authorized | D-79, D-168 |
| Instruction files | Explicit instruction sources are read at the configured boundary | D-102, D-246 |
| State paths | State-root kind and path validation applies before writes | D-229 |
| Skills roots | Skills live in configured roots and load on demand | D-232 |
| Defaults | Current defaults remain explicit | D-239 |
| Retry budget | Transport retry counts are bounded per instance | D-240, D-247 |
| Budget ceiling | Usage ceilings refuse new requests rather than silently exceeding them | D-287 |
| Verification | Independent verification follows producer/verifier authority and evidence boundaries | D-393, D-395 |
| Tool timeouts | Model-picked and configured timeouts remain bounded | D-414, D-415 |
| Context limits | Tool-output and retained-context caps preserve bounded requests | D-426 |
| Repository config | Repository-local config is discovered only through explicit trust | D-244, D-434 |
| Input pages | Input reads enforce row and byte budgets without silent mutation | D-452 |
| Update | Governed update requires an exact artifact and user approval | D-453 |
| Delegation | Only explicitly delegable grants can produce attenuated task-scoped children | D-455 |
| Reviewer | Automatic review is strict, scoped and cannot approve self-update | D-458 |
| Prompt cache | The L1 frontier is stable between compactions and telemetry is provider-reported | D-459 |
| Diagnostics | Read-only diagnosis uses the same readiness rule as execution | D-461 |
| Formal coverage | Claims need a checked root, a refuting control or an explicit type/status exception | D-462 |
| Reasoning effort | Effort selection falls back to the nearest declared lower level | D-476 |
| Environment/input | Environment and pending-input observations preserve identity and revision | D-479 |
| Durable work | Work persists across turns, compaction and restart with bounded continuation | D-487 |
| Images | Image support follows the model's declared capability | D-392 |
| Search tree | One revisioned session exploration tree accompanies compaction | D-504 |

## Current rules

### Authority and runtime

- `core/src/v2/control.rs::Control::submit` is the single team-transaction entry point. Identity, operation ids,
  permission revisions and event sequence numbers come from the control plane, not from model or client fields.
- Each session has one authoritative SQLite store with WAL and `synchronous=FULL`. Engine writers serialize
  through the bounded single-writer worker. Model and tool waits happen outside transactions.
- Execution state is driven by the phase machine and persisted location. Known results are reused; unknown
  effects are marked unknown and are never replayed on a guess.
- User-only instance termination remains distinct from interrupt, task cancellation, pause, detachment and
  daemon shutdown. Cancelling a task does not prove that its process or remote effect stopped.
- Capability grants can only narrow their parent. Revoking a parent revokes derived grants. A model checkpoint,
  repository instruction, tool result or tree node cannot grant authority.
- Ordinary Shell, acceptance commands and local MCP run on the host. This is not OS confinement; same-user
  processes can access whatever their OS permissions allow. Retired sandbox settings are rejected.
- Self-update requires a concrete user approval. An automatic reviewer can assess only a matching operation,
  with strict evidence and bounded current-user/policy context; it cannot approve self-update.

### Work and collaboration

- Work scopes own budget, deadline, admission and acceptance. Work states and tasks own continuation,
  delegation and attribution. Sessions own history and isolation. There is no separate goal entity.
- Work continues across turns, compaction and restart. Progress prose is not completion. Empty or repeated
  unproductive recovery is bounded; blocked, failed and cancelled outcomes are not successes.
- User-defined required checks must pass. A green command alone does not prove the whole deliverable.
- Delegation uses explicit task/result paths and scoped grants. Members have no ambient Shell grant.
- The user may read any instance's history and address any member. An instance may read only its own private
  history; shared projections must not become a side channel into another member's private context.
- Workspace policy is shared, isolated or Git worktree. Workspace retirement refuses uncommitted or unmerged
  work. Shared-workspace takeover requires separate stop evidence rather than assuming cancellation stopped it.

### Context, models and exploration tree

- Model requests use each profile's declared native context window. A smaller synthetic window is allowed only
  for scripted regression tests, never as evidence for a real-model evaluation.
- Compaction preserves the original request, user revisions, acceptance criteria, open questions and receipt
  references. Covered entries remain retrievable in stored history.
- There is one session-level exploration tree. It is model-maintained navigation, not per-instance state,
  authoritative history, completion evidence, capability authority or a way to rewind side effects.
- Only the persisted Leader writes the shared tree. Leader compaction advances it from the previous cumulative
  tree and newly visible Leader entries through the retained recent tail. Member-private history is excluded.
- Tree reads are User/Leader operations. Atomic patches support add, update, move, merge and focus, with a
  current base revision and stable operation identity. A matching replay does not grow history and reconstructs
  control-assigned `added_ids` when needed.
- Ordered operations validate against the preceding operation's result. Any invalid successor rolls back the
  entire patch. Tree nodes, text and references are bounded: at most 96 nodes, 32 operations per patch and 32
  levels. Omitted nodes survive; abandoned branches and old revisions remain inspectable.
- Freshness compares the complete `(instance, epoch, entry)` watermark, including empty epochs. Invalid or
  stale compaction patches emit `search_tree_update_failed`; the summary and user's turn still continue.
- `/tree`, `teamagents search_tree [--json]`, daemon reads and session export expose the same projection.
  Human output escapes terminal controls, embedded line breaks, Unicode line separators and bidi/isolate
  formatting controls; stored JSON retains the normalized content.

### Providers, integrations and security

- Chat Completions, DeepSeek, Responses and Anthropic adapters can coexist in one session. A member's model
  and reasoning effort apply at its next request, not retroactively to an in-flight request.
- Provider attempts separate logical request ids from transport attempt ids. A complete late response can be
  selected once; partial or unselected responses never create tool effects.
- Stream framing, visible-text boundaries, retry ownership, stall detection and response-size limits are
  explicit. A timeout is not evidence that no side effect occurred.
- MCP supports stdio and streamable HTTP, bound through the same authorization, budget, cancellation and
  receipt paths. Resource list/read uses existing service bindings and rejects ambiguous multi-service reads.
- Skills and repository instructions are read on demand and cannot grant capabilities. Hooks may veto native
  tool calls or report events; project-trusted configuration cannot silently widen authority.
- Dependency advisories and unknown sources fail CI. Credentials remain in environment variables or the
  machine's own stores and never enter source, prompts, events or logs.
- OAuth/MCP/update inputs are bounded, and credential writes are locked and atomic.

### Interface and session management

- One daemon per session owns the JSON-lines Unix-socket protocol. TUI and headless clients resume from the
  event watermark; no second execution engine runs inside the TUI.
- The TUI has one Leader conversation with elevated prompts, grouped tool activity, conditional approvals,
  Markdown/code/media rendering and a conditional live activity row. It has no member topology canvas.
- Enter submits; Shift+Enter inserts a newline. Ctrl+C interrupts the model request, Ctrl+Q detaches,
  Ctrl+S opens sessions, Ctrl+P opens pending inputs, Ctrl+M opens model/effort selection, and `/tree` opens
  the session exploration tree.
- Named sessions support history, search, fork, rename, archive, restore and guarded deletion. A fork requires
  a stopped source daemon and fresh execution evidence. Binary rollback alone does not undo a schema migration.
- Local export streams stored history, revisions and references from one read snapshot. It never overwrites an
  existing destination and preserves explicit provenance.

### Current verification boundary

- Current local regression contains 773 Rust tests: core 190, engine 478 and TUI 105.
- Formal material has 393 configurations across 48 modules: 65 positive configurations, 327 negative controls
  and one separate wide configuration. Positive inputs share 62 traversals only when model/spec/domain match.
- There are 79 normal-path witnesses and 9 Kani harnesses. Witnesses demonstrate reachability, not safety.
- Search-tree verification includes ordered pair transitions, 23 controls covering every non-`TypeOK` tree
  claim and 11 witnesses. `TypeOK` remains a type-level claim, not a behavioral completeness proof.
- These results do not establish full Rust/SQLite refinement, whole-system composition, Policy B source-bound
  equivalence, wide exhaustive completion, arbitrary-input TLA coverage or live-model navigation quality.

## D-508 Keep only current project state and tools (2026-11-12)

The tree contains the current Rust implementation, current documentation and executable development/verification
tools. `review/` has been removed; required utilities are under `tools/`. No review reports, evaluation datasets
or archived evidence are required by the current source or its gates. Current documents describe only current
behavior, current evidence and open obligations.

Current search-tree coverage includes one session tree, ordered two-operation patches, 23 controls covering every
non-`TypeOK` tree claim and 11 witnesses. Implementation checks additionally cover multi-operation ID ordering,
arbitrary Unicode and terminal controls, compact JSON variants, real SQLite revision/head/event rollback,
schema-migration conflict refusal and random control-walk tree invariants. The original 24-step executable
control-plane walk is retained before explicit tree read/write steps, so tree coverage does not displace approval
coverage. These remain bounded checks, not arbitrary-input TLA enumeration, full refinement or live-model
navigation-quality evidence.
