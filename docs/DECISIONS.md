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
  with strict evidence and bounded current-user/policy context; it cannot approve self-update. Quoted genuine
  user requests remain authorization evidence; operation arguments and claimed prior approvals do not. The
  deterministic refusal layer recognizes publishing commands, credential/runtime access and privilege escalation;
  literal read-only searches for those words remain model-reviewable, while shell syntax that can execute or
  redirect is still refused conservatively.

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
- MCP supports revision `2025-06-18` over stdio and streamable HTTP, bound through the same authorization,
  budget, cancellation and receipt paths. Resource list/read retains resource-only services and rejects
  ambiguous multi-service reads. Tool discovery is bounded across pages and rejects repeated cursors.
- Both artifact collectors protect retained context and operation-output references, retry failed deletion
  claims and skip `EVIDENCE` roots. Releasing an owner does not expire retained output bytes.
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

- Current source contains 814 Rust tests: core 197, engine 509 and TUI 108.
- Formal material has 393 configurations across 48 modules: 65 positive configurations, 327 negative controls
  and one separate wide configuration. Positive inputs share 62 traversals only when model/spec/domain match.
- There are 79 normal-path witnesses and 9 Kani harnesses. Witnesses demonstrate reachability, not safety.
- Search-tree verification includes ordered pair transitions, 23 controls covering every non-`TypeOK` tree
  claim and 11 witnesses. `TypeOK` remains a type-level claim, not a behavioral completeness proof.
- These results do not establish full Rust/SQLite refinement, whole-system composition, Policy B source-bound
  equivalence, wide exhaustive completion, arbitrary-input TLA coverage or live-model navigation quality.

## D-516 Preserve input ownership and recover client refresh failures (2026-11-14)

Control registration events freeze the instance epoch, context head and billing work; user-input application
events identify the exact position for both immediate and queued delivery. Reviewer registration identifies
the operation's originating request. These fields come from transaction state, never client ownership claims.
Headless accounting selects Leader/compression requests between the input and its first applicable outcome,
member requests newly registered in the same billing work during that interval, and reviews of selected
requests. Prior requests and other works are excluded even when their attempts arrive later. Batched inputs
and overlapping follow-ups can share calls; this reporting does not allocate exclusive causal costs or change
the authoritative work ledger. Pending requests, missing receipts and uncertain cancellation remain incomplete.
The collector retains compact receipt data, not streamed text or complete tool histories. It caps retained
accounting at 10,000 registrations and a 16 MiB bookkeeping budget; overflow reports incomplete outcome/usage
instead of dropping evidence. Outcome events trigger history attribution even if a later input already has
the Leader busy, so continuous input cannot hide an earlier answer or extend its accounting interval.

History attribution follows older pages with strictly decreasing cursors and retains only the input identity
and one candidate outcome. Read errors are infrastructure failures, not timeouts. A timeout cannot claim
complete cost. TUI history refresh remains pending after transient failure even if other socket calls succeed.
An oversized event returns an explicit resync result without acknowledging it; ordinary checkpoint refresh
still preserves unread cursors. These are executable regression claims, outside the bounded daemon model.

## D-515 Keep literal publication searches reviewable (2026-11-14)

A restricted literal `rg`/`grep` command with explicit pattern and paths may pass the deterministic publishing
keyword check so the Reviewer can assess it. This does not auto-allow the operation. Credential, runtime,
privilege and self-update guards still apply. Shell composition, redirects, expansions, preprocessing and
unsupported syntax retain conservative refusal. Genuine user requests remain authorization evidence when
quoted; arguments and claimed prior approvals grant none. Strict complete Reviewer JSON is still required.

The DeepSeek V4 rerun matched 34/34 fixed labels after this correction. It reused the development and earlier
holdout cases and is regression evidence, not an independent holdout. Claude V3 matched 24/34 labels: one
benign deterministic refusal, four strict-parser errors and five HTTP 402 quota failures account for the rest.
Those operational failures are not correct refusals; the unavailable paid route was not retried.

The portable Rust Reviewer probe records fixed input hashes, compiled source and executable hashes, native-window
provenance, per-case usage/errors and honest completion counts. It never executes proposed operations; dry runs
make no provider calls. Transport/timeout failures stop the remaining calls for that profile, and existing
output files cannot be overwritten. A separate DeepSeek rerun through this probe also matched the same 34/34
fixed labels. No frozen evaluation dataset or historical receipt is required by the source gates.

## D-514 Keep maintenance and update compatible with actual CLI contracts (2026-11-14)

Normal clients require socket protocol 2. A maintenance connection accepts protocols 1 and 2 only for checkpoint
and shutdown, without a command id; other methods and future versions are refused. A failed greeting is not
proof that a daemon stopped: only a refused or missing socket permits that conclusion. Update health requires a
valid checkpoint. Binary version discovery reads the CLI's JSON `version` field and retains the older plain
`teamagents VERSION` format, refusing invalid path-like labels. Old installed updater code cannot acquire these
fixes automatically; an extracted, verified new binary can perform the managed transition explicitly.

Explicit rollback validates the database before switching. Failed startup terminates and reaps only the child
it launched, then attempts to restore the starting version. Target, recovery and journal failures remain in the
returned diagnostic; a failed recovery records `failed`, while a successful restoration records `rolled_back`.
Startup polling leaves stale-socket cleanup to the daemon under its coordinator lock; uncertain connection
errors do not authorize unlinking. `--no-restart` leaves the running daemon alone during apply. Each update or
rollback captures its actual starting directory. Recovery restores `current` directly even if an unrelated bin
link is obstructed, and a same-release reapply does not restore a stale `previous` target. Switch and post-stop
journal failures enter the same recovery path; secondary link/journal errors remain visible.

## D-513 Retain MCP resources and bound discovery and authentication (2026-11-14)

The negotiated MCP revision is exactly `2025-06-18`; missing or different revisions fail explicitly. Bound
services survive independently of selected tool rows, including resource-only peers and configurations that
filter out every tool. Codemode advertises and executes their resource operations through the existing binding
and hook checks. Tool discovery follows cursors with one total discovery deadline, at most 100 pages and a
16 MiB aggregate response budget; malformed or repeated cursors fail. Server requests preserve string or numeric
JSON-RPC IDs. HTTP authentication installs the refresh callback before initialization, and a 401 forces token
refresh even when the recorded expiry is still in the future. Refresh requests inherit the remaining discovery
budget, and the retry recomputes what remains after refresh. Stored OAuth credentials must match the exact
resource string recorded by login; validation, selection and refresh share one credential lock. Waiting for
that lock consumes the operation budget. Codemode validates the normalized catalogue name and dispatches the
original remote name. Credential locking and atomic writes remain. These checks do not establish full MCP conformance, external OAuth
interoperability or stream resumability.

## D-512 Preserve referenced artifacts and retry eligible collection (2026-11-14)

An operation's nonnull `output_ref` must name a LIVE artifact in the same session, published atomically with its
receipt. Consuming the receipt copies the reference into context. Both claim and collection also inspect stored
operation receipts, protecting old roots whose context entry omitted the reference. Owner release alone never
makes those bytes collectible. New references cannot attach to DELETING artifacts. A sweep returns newly
`claimed` ids and all eligible `pending` deletions, so a failed or interrupted unlink can retry without a duplicate
claim event. Referenced old DELETING rows are excluded before touching bytes. Online and offline collectors skip
`EVIDENCE` roots, including pending deletion retries. This restores existing reference protection; it introduces
no destructive expiry policy and cannot restore bytes already deleted by an older collector.

## D-511 Reduce request construction overhead without changing the request (2026-11-14)

Kernel materialization clones each wire message once and stops looking for tool results when every required
answer has been placed. Token estimation counts the identical compact JSON representation through a bounded
counting sink, preserving aggregate ASCII rounding, Unicode scalars, escaping and image allowances. The
messages/tools estimator borrows the existing arrays. Tools, prompts, masking policy, admission and native
context windows are unchanged. Serialized-reference checks cover generated Unicode/control text and images;
a source comparison against the preceding implementation covers text, tool-heavy, missing/repeated-result and
image requests. These changes reduce local construction work, not provider token use or task-quality criteria.

## D-510 Freeze optional model tariffs and report incomplete cost (2026-11-14)

The user's remaining-work instruction includes configurable monetary accounting. Model profiles may supply a
USD-per-million tariff with explicit input/output rates, optional cache read/write rates and a source/date.
Request registration freezes that tariff for normal, compression and Reviewer calls; attempt receipts calculate
cost from the frozen rates and provider-reported categories. Nullable additive schema-9 columns preserve old
rows as unpriced. Missing usage, missing rates and ambiguous model-profile lookup never imply zero cost.
Work totals include attributed admission requests and expose pending requests and usage without local receipts.
The headless report exposes the known subtotal and completeness, and exports retain tariffs and attempt costs.
Known usage from failed Reviewer calls and semantically invalid summaries remains receipted. Invalid summaries
still fail compression while preserving the paid transport response and raw artifact. Closed requests without
a receipt cannot appear free. Only explicitly selected catalogue keys bypass wire-name ambiguity checks.
These are user-configured estimates, not invoices or monetary admission limits; token admission is unchanged.

## D-509 Bound daemon pages and preserve delivery cursors (2026-11-14)

The user selected actionable remaining work, including large-history limits, cost measurement and external
acceptance. Socket protocol 2 separates delivered event positions from snapshot/log heads. Event pages have
row and byte limits, fixed-end drains and explicit reclaimed-range resync. Initial/resync checkpoint adoption
is separate from ordinary refresh, so unread events are not acknowledged. Headless completion drains all pages
through its sampled head before reporting usage or a terminal outcome. Incompatible protocol 1 clients fail
explicitly; schema 9 and work policy 1 are unchanged. Socket frames are bounded at 32 MiB, and history pages
have a byte budget and an older-page cursor. The existing repository-trust and artifact ownership policies remain
binding. A local directory-isolated installation is not a new-machine acceptance result.

## D-508 Keep only current project state and tools (2026-11-13)

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
