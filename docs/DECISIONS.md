# Current implementation decisions

This file records the rules currently in force, not their change history. Stable rule IDs identify current
contracts and may be cited from source or documentation; they do not require keeping superseded implementations
or experiment reports. The baseline is [DESIGN.md](DESIGN.md), acceptance is in [ACCEPTANCE.md](ACCEPTANCE.md),
and formal limits are in [verification/REPORT.md](../verification/REPORT.md).

## Current rule index

| Topic | Current contract | Rule IDs |
|---|---|---|
| Providers | Compatible services use configured profiles | D-40 |
| Daemon identity | Clients learn authoritative session/workspace identity from the daemon | D-41, D-57 |
| Headless outcomes | Each run reports its own outcome and attributed usage | D-49, D-72, D-516 |
| No ambient Shell | Spawned instances need explicit scoped grants | D-61 |
| MCP | Bound tools/resources share authority, budgets and receipts; discovery/authentication are bounded | D-74, D-513 |
| Unsupported configuration | External execution profiles and sandbox settings are explicitly refused | D-75, D-161, D-480 |
| Web bindings | Search/fetch require configuration and authorization | D-79, D-168, D-482 |
| Instruction files | Explicit sources load at the configured boundary | D-102, D-246 |
| State paths | State-root kind and path validation precedes writes | D-229 |
| Skills | Configured roots are validated and content loads on demand | D-232 |
| Defaults | Defaults remain explicit | D-239 |
| Retries | Transport retries are bounded per instance | D-240, D-247 |
| Repository config | Explicit project trust is required; policy remains user-only | D-244, D-434 |
| Budget ceiling | Usage ceilings refuse requests rather than silently exceeding limits | D-287 |
| Formal scope | Claims require a checked root and control, or a declared type/status exception | D-362, D-462 |
| Images | Requests follow the model's declared image capability | D-392 |
| Verification | Independent checks respect producer/verifier authority | D-393, D-395 |
| Deadline warning | One warning per live work, before its deadline | D-410 |
| OAuth | HTTP services use explicit login and resource-bound credentials | D-412 |
| Tool timeouts | Model-picked and configured timeouts remain bounded | D-414, D-415 |
| Check briefing | Required checks are stated once per live turn | D-418 |
| Context limits | Tool-output and retained-context caps bound requests | D-426 |
| Retention | Explicit pruning windows and reference protection govern collection | D-435, D-436, D-512 |
| Input pages | Row/byte budgets do not silently mutate inputs | D-452 |
| Update | Exact artifacts and user approval are required; rollback preserves recovery errors | D-453, D-514 |
| Delegation | Only delegable grants produce attenuated task-scoped children | D-455 |
| Reviewer | Strict scoped review cannot approve self-update; literal searches remain reviewable | D-458, D-515 |
| Prompt cache | The L1 frontier is stable between compactions; telemetry is provider-reported | D-459 |
| Diagnostics | Read-only diagnosis uses execution's readiness rules | D-461 |
| Reasoning effort | Selection falls back to the nearest declared lower level | D-476 |
| Environment/input | Observations preserve identity and revision | D-479 |
| Durable work | Bounded continuation persists across turns, compaction and restart | D-487 |
| Search tree | One revisioned session tree; atomic ordered patches and supplied source-entry IDs | D-504, D-506, D-517 |
| Repository contents | Keep current implementation, documentation and runnable tools; Git retains history | D-508 |
| Daemon pagination | Bounded pages preserve delivery cursors and explicit resync | D-509 |
| Billing | Priced estimates, tariffs and cost receipts are removed from the runtime and its reports | D-537 |
| Request construction | Borrowed estimation preserves wire content and token-count semantics | D-511 |
| Tool arguments | Malformed calls fail before approval/dispatch; valid sentinel-key data remains valid | D-518 |
| Source inventory | Cargo lists determine current counts, not passing-gate claims | D-519 |
| Format versions | Stated schema/protocol/policy versions and their migration chain are gated against the constants | D-539 |
| Advisory exceptions | Reviewed `ignore` entries are gated in both directions: a stale exemption and an unreviewed unmaintained advisory both fail | D-540 |
| File edits | Supplied hash preconditions validate before staging or writes | D-520 |
| Target ABI | File-open flags use target constants; cross-emulation is not native acceptance | D-521 |
| Session safety | Listening or uncertain endpoints prevent destructive lifecycle actions | D-522 |
| Profile selection | Resolve profiles before preparation/recovery; cached retries stay unchanged | D-523 |
| Team interface | Member observation is separate from Leader action ownership | D-524 |
| Public fetch | Fake-IP hostnames require public-address verification; content caps count UTF-8 bytes | D-525 |
| Workspace sessions | Ordinary startup restores the most recently opened session in the launch workspace, creating one if absent | D-526 |
| Interface contracts | Validate scoped options before state access; share command metadata; preserve intentional key actions and resolved overrides | D-527 |
| Session model roles | User selects one Leader model and one shared Worker policy | D-529 |
| Selected workflows | Manual Leader compaction and after-work input; scoped native graph reads and authenticated search caching | D-530 |
| Leak-guard ownership | Only processes that inherited this run's marker are this run's leaks | D-528 |
| Interface feedback | Command answers are transient feedback or a dismissible overlay; the conversation holds only the model exchange and input receipts | D-531 |
| Interactive workspace UX | TUI modals (`Ctrl+K` palette, `/rewind`, `/mcp`, approval drawer), interactive tree/wall/pinning, confirmed member/task interventions, single-shot direct message, search/OSC 52 copy, and multi-session monitor | D-532 |
| Command identity | Replay dedup hashes canonical JSON; legacy receipts still match | D-533 |
| Damaged state | Unreadable stored wait conditions never satisfy a wait and are reported | D-534 |
| Idle polls | A poll that changed nothing keeps no receipt, for every poll command | D-535 |
| Driver failure | A driver that ends in error is parked with its reason, never silently respawned | D-536 |
| Tool surface & onboarding UX | Concise single-word native tool names with legacy aliases; unified Ctrl+A capability approvals, Ctrl+K recovery/search, and clickable beginner onboarding | D-538 |

## Authority and runtime

- `core/src/v2/control.rs::Control::submit` is the single team-transaction entry. Identity, operation IDs,
  permission revisions and event sequence numbers come from control state, not model/client claims.
- Each session has one SQLite store with WAL and `synchronous=FULL`. Engine writes serialize through a bounded
  single-writer worker; model/tool waits happen outside transactions. The store uses schema 14 with explicit migration.
- Persisted phase and location drive execution. Known results are reused; unknown effects become
  `OUTCOME_UNKNOWN` and are never replayed on a guess.
- Instance termination is user-only, distinct from interrupt, cancellation, pause, detachment and shutdown.
  Cancelling a task does not prove that its process or remote effect stopped.
- Grants only narrow their parent, and parent revocation cascades. Instructions, tool results, model checkpoints
  and tree nodes cannot grant authority. Ordinary Shell, acceptance commands and local MCP run on the host;
  logical permissions are not OS confinement. Unsupported sandbox/external execution settings are refused.
- Self-update needs concrete user approval. Automatic review assesses only the matching operation with strict
  complete JSON and bounded current-user/policy context. Genuine user requests remain authorization evidence;
  arguments and claimed prior approvals grant none. Credential/runtime access and privilege escalation retain
  deterministic guards. Restricted literal searches may reach model review; executable shell composition,
  redirects, expansions and unsupported syntax remain conservatively refused.
- Malformed JSON, non-object arguments and non-string argument encodings fail before approval/dispatch with
  failed, not-started receipts. Raw calls and known usage remain recorded. `_invalid_arguments` is valid data;
  caller-written parse metadata is ignored. PREPARED intents use stored source calls where available, while
  dispatched recovery retains result/unknown-outcome rules. This is not complete per-tool schema validation.

## Work, accounting and collaboration

- Completion accepts a successful operation ID copied directly from the work observations as well as a prefixed receipt reference. Both resolve against the same instance, work and task.
- Work scopes own budget, deadline, admission and acceptance. Work states/tasks own continuation, delegation
  and attribution; sessions own history and isolation. There is no separate goal entity.
- Work continues across turns, compaction and restart. Progress prose, valid continuation checkpoints and
  repeated observations alone neither complete nor block enrolled work. The next turn receives feedback;
  empty replies and rejected protocol or completion actions retain bounded recovery. Without a budget or
  deadline, valid but unproductive turns may continue until interrupted. Blocked, failed and cancelled
  outcomes are not successes. Required checks must pass, but a green command alone does not prove delivery.
- A first tool decision for matching, live pending input admits its work before preparing operations in the
  same transaction. Plain informational answers still create no work; execution uses the ordinary dispatch
  checks. The frozen request scope stays unchanged; the operation and its receipts belong to the new work.
- Model task calls forward explicit capabilities. A user-only default task capability list applies when
  omitted; an explicit empty list opts out. Neither form grants authority.
- Members have no ambient Shell grant. Delegation uses explicit task/result paths and scoped grants.
- The user may read/address any instance. An instance reads only its own private history; shared projections
  must not leak another member's context. Shared, isolated and Git-worktree policies are supported. Retirement
  refuses uncommitted/unmerged work; shared-workspace takeover requires independent stop evidence.
- D-537: billing is removed. There is no `[models.<name>.price]` table, no `PriceRates`, no
  `model_requests.pricing_json` or `attempts.cost_json` (schema 14 drops both), no work-cost view and no
  `usage.cost` object in headless or exported reports. Earlier builds' priced estimates are not migrated,
  reinterpreted or shown: the columns are gone rather than left unread, so no surface can imply a priced
  estimate still exists. Token usage — prompt, completion, total and the provider-reported cache split — is
  still recorded per work and remains the accounting evidence; a missing usage report is counted
  (`unpriced_attempts`) so a partial token total never reads as complete. Token budgets and deadlines are
  unaffected: they never were monetary limits.
- Input/request attribution uses control-assigned epoch, head, work and Reviewer-parent positions.
  Headless accounting includes calls between application and the applicable outcome, newly registered member
  calls in that work and their reviews. Prior/unrelated calls are excluded. Overlapping inputs may share
  requests; these are not exclusive causal allocations. Pending/missing evidence and uncertain cancellation
  stay incomplete.
- Accounting retains compact receipts, capped at 10,000 registrations and 16 MiB. Overflow reports incomplete
  outcome/usage rather than silently dropping evidence. Outcome events trigger attribution even when a later
  input is active. Older-history cursors strictly decrease; read errors are infrastructure errors, not timeouts.

## Workflow contracts and scope

- D-530: support rewind, not general message branching. Manual Leader compaction is a maintenance request,
  not a chat turn. Ordinary input steers current work; explicitly held Leader input enters only after the
  named work settles, including cancellation, at a boundary without another work owner. Held checks never
  attach to an intervening work. A blocked but active scope is not settled.
- Persistent Workers stay available by default. An explicitly temporary Worker parks after its final settled
  task at a quiet boundary; the driver exits and closes its MCP clients. Release preserves the instance,
  workspace, history and receipts. Cancellation or a blocked task does not trigger automatic release.
  A Worker finish belongs to the assignment recorded for its request, not to the queue head at delivery.
  This is not permission for a member to terminate another instance.
- Target official OpenAI, Anthropic and Google subscription integrations only, when a supported third-party
  path is available. Model account credentials are separate from MCP credentials. Never import another harness's
  auth store. Typed classifiers, image generation/editing and actual image return require protocol-specific
  acceptance; a compatible chat endpoint or an image preview is not equivalent.
- Session MCP controls apply to an exact configured key. Start/stop control connections; hide/expose control
  both tool and resource access, independently of connection state. Stop suppresses even a required service by
  explicit user choice; start and reconnect do not unhide it. Forks inherit restrictions. A policy receipt
  records desired state, not a claim about live clients. Refuse changes while a model request, operation or
  unresolved dispatch might use the old surface. No user configuration or credentials are edited.
- The operator shell runs in the attached workspace and is not a model tool. Output is local unless the
  user explicitly stages it as a draft, reviews it, then sends it.
  Skip new import/export formats, virtual routing, plugin/SDK parity and GGUF downloads. Defer additional
  platforms and host updater/proxy migrations; do not install timers or migrate user accounts.
- Catalogue refresh is both scheduled and on demand. Preserve user edits, update only managed fields, mark
  missing models without removing them, retain differences and rollback history, and preserve the last good
  catalogue on failure. Only new requests may observe a refresh; never silently substitute another model.
- Rewind has exactly two modes: conversation only (default), or conversation and tracked files. Preserve
  the abandoned branch and editable prompt. Preview file changes and refuse conflicts or unconfirmed stop;
  never rewind usage, approvals, task facts or external effects. File-only rewind is not supported.
  The conversation-only implementation captures delivered Leader inputs, starts a new epoch in the same
  session and reopens the original editable payload; it never resubmits it automatically. The combined mode restores the working bytes of Git-tracked regular files at Leader prompt delivery,
  including dirty files later changed by shell commands without native edit receipts. The preview labels
  unattributed differences and binds confirmation to the observed file digests and conversation point.
  A later mismatch with a recorded native edit blocks restoration. A missing delivery snapshot leaves
  only the read-only receipt comparison available. Interrupted native file mutations with a committed dispatch
  and missing receipt close as `OUTCOME_UNKNOWN`. This preserves any intervening operator edit.
- Native CodeGraph serves an explicit project allowlist via MCP. Report repository identity and index
  freshness without widening a member's scope, creating an index or inventing protocol compatibility.
- A script is not a blanket approval. Effectful subcalls require their own operation, authorization, receipt
  and cancel/recovery checks. Do not replay unknown effects or roll back facts about completed subcalls.
- Per-request routing is confined to explicitly allowed destinations, capabilities and budgets. Keep prior
  attempts and their known/unknown charges. Visible text or unknown side effects prevent automatic replay.
  Managing local model files and serving processes requires explicit user authorization, not inference.
  The local-router CLI reads exact user profile keys only. Load/unload require explicit confirmation;
  loss of a router acknowledgement is not a reason to resend. No session tool or discovery entry grants
  router management access.
- Native Apple Silicon distribution is deferred. These requirements describe the target, not passing evidence.

## Context, profiles and exploration tree

- Startup selects an explicit `--model`, the saved Leader selection, or the user-owned `default_model`, in that order.
  Without a selection it uses the `leader_main` starter key or the sole configured model. Multiple remaining choices require an explicit key.
- D-529: the user selects an exact catalogue key for the Leader. All Workers follow that selection by
  default, or use one explicitly pinned key, selectable before any Worker exists. Pinning the Leader's
  current key is not following. Search filters catalogue metadata locally; only confirmation selects a key.
  Neither a member ID nor a spawn argument can select a private Worker model. This does not change grants,
  work ownership, instructions, or the separate Reviewer binding.
- Role selection and Worker creation serialize in control. A switch affecting a busy instance fails as a
  whole, leaving prepared requests and retries untouched. Terminated instances retain their last profile.
  Migration does not infer intent from matching wire names or rewrite historical requests. Legacy live Workers
  without a shared policy require explicit follow or pin before new Worker requests or spawns. An unknown
  Leader route requires explicit Leader selection before it can serve as a follow target.

- Real-model evaluation uses each profile's native context window with recorded provenance. Synthetic small
  windows are for scripted regressions only.
- Persisted profiles reload after the post-drain snapshot, before preparation, compaction and reservation.
  Catalogue keys resolve to wire names, defaults and normalized effort; revision checks reject stale preparation.
  Explicit selection binds the adapter by catalogue key, not a possibly shared wire name. Cached
  retries retain their prepared request. Recovery checks both route and serialized provider-neutral request
  (including the tool surface and expanded images); a missing or mismatched digest parks without a resend.
  Automatic routing, model-account login and live-service acceptance are separate obligations.
- Materialization preserves wire messages/tool-result pairing. Bounded compact-JSON estimation preserves
  aggregate ASCII rounding, Unicode scalars, escaping and image allowances without duplicating input arrays.
- Compaction preserves the original request, revisions, acceptance criteria, open questions and receipt
  references. Covered entries remain readable. Summarized and retained entries carry JSON-quoted stored IDs
  before input caps. Optional source references are model-written navigation, not validated semantic evidence.
- One session-level exploration tree accompanies Leader compaction. It is not per-instance state, authoritative
  history, completion evidence, capability authority or a way to rewind side effects. Only the persisted Leader
  writes the shared tree; member-private history is excluded.
- Reads are User/Leader operations. Atomic add/update/move/merge/focus patches require current base revision and
  stable operation identity. Matching replay does not grow history and reconstructs control-assigned `added_ids`.
- Ordered operations see prior results; any invalid successor rolls back the entire patch. Limits are 96 nodes,
  32 operations per patch and 32 levels. Omitted nodes survive; revisions and abandoned branches remain readable.
- Freshness compares `(instance, epoch, entry)`, including empty epochs. Invalid/stale compaction patches emit
  `search_tree_update_failed`; the summary and turn continue. `/tree`, CLI, daemon and export expose the same tree.
  Human output escapes terminal, line-break and bidi/isolate controls; JSON retains normalized content.

## Tools, providers and integrations

- File access uses target `libc` constants for directory/nonblocking opens. Supplied `expected_sha256` values
  must be 64-character hexadecimal strings; malformed values fail a whole batch before staging/writes. Hashes
  remain optional and case-insensitive for both batch forms. Tool-body hash failure is distinct from predispatch
  argument refusal. Path pinning, authority, atomic commit and rollback rules remain binding.
- Chat Completions, DeepSeek, Responses, Gemini and Anthropic adapters coexist. Profile changes apply at the next
  request, never retroactively. Logical requests and transport attempts are distinct; a complete late response
  may be selected once, while partial/unselected responses cannot create tool effects. Retry, stream framing,
  stall and response-size limits are explicit. Timeout does not establish absence of side effects.
- D-482: Search and fetch each require their own provider bindings. Automatic fetch tries bound Exa first,
  then bound DeepSeek only on a transient transport/read failure or HTTP 408, 429 or 5xx. Explicit selection
  never switches providers. Missing/rejected credentials, redirects, invalid input, malformed responses and
  guard failures do not trigger fallback. Each attempt uses only its binding's credential.
- The URL guard canonicalizes HTTP(S), rejects credentials/internal hosts/non-public literals and checks all
  local DNS answers. Fake-IP hostname answers in `198.18.0.0/15` require public A/AAAA verification through
  Cloudflare HTTPS DNS, with Google fallback. Both families must finish; known private answers stop fallback.
  Empty/malformed/failed verification never authorizes fetching. Fake-IP literals and mixed private answers
  stay blocked. TLS remains enabled, redirects are disabled for DNS requests and no provider credentials are sent.
- DNS verification shares a ten-second deadline and 64 KiB body cap. At most eight uncancellable OS lookup
  workers may linger. A resolver may consume the whole deadline. Fetch content is clipped at a UTF-8 byte
  boundary. Provider-side DNS, redirects, later rebinding and broader proxy environments remain external obligations.
- Generic MCP negotiates exactly `2025-06-18` over stdio/streamable HTTP. Only the user-bound native
  CodeGraph stdio client uses `2024-11-05`, defaults to explore, allows explicitly selected scoped read queries and refuses resource capability.
  Bound resource-only/filtered services remain
  available through normal authorization/hooks. Discovery shares one deadline, 100 pages and 16 MiB aggregate
  response budget; malformed/repeated cursors fail. JSON-RPC string/numeric IDs are preserved. Codemode checks
  normalized catalogue names and dispatches original remote names; ambiguous resource reads are refused.
- HTTP refresh is installed before initialization; a 401 forces refresh. Refresh, credential-lock waiting and
  retry consume the same remaining budget. Stored credentials match the exact login resource; validation,
  selection and refresh share a lock. Writes are atomic. Full external OAuth/MCP conformance and stream
  resumability remain open.
- A nonnull operation `output_ref` names a same-session LIVE artifact published atomically with the receipt.
  Receipt/context references protect bytes after owner release. DELETING artifacts reject new references;
  referenced deleting rows are excluded from collection. Eligible pending deletions retry without duplicate
  claims. Online/offline collectors skip `EVIDENCE` roots. No destructive reference-expiry policy is selected.
- Skills/instructions load on demand and cannot grant capabilities. Hooks may veto calls or report events;
  repository config needs explicit trust and cannot override user-only hooks/checks/limits/retention/update policy.
  Dependency advisories and unknown sources fail CI. Credentials stay in environment or machine stores, never
  repository source, prompts, events or logs.

## Interface, sessions and updates

- CLI options are validated for the selected command and subcommand before session resolution or mutation
  (D-527). Unsupported options and conflicting updater actions fail explicitly. `--` ends option parsing;
  MCP argv values can start with a dash. Top-level help stays short, with complete scoped command help.
  Compatibility `--full-auto` and MCP `--execution host` remain accepted without advertising an empty choice.
  Explicit permission overrides feed the same resolved settings used by daemon boot. Accepted JSON mutation
  options produce JSON receipts. Startup settings that cannot apply to an existing session are reported.
- Default keyboard bindings remain. Action keys require their documented modifiers; navigation and editing
  may repeat, but confirmations and mode changes do not consume Repeat events (D-527). Approval selection
  follows identity across refreshes. `/edit [TEXT]` opens the external editor; Ctrl+E edits the current draft.
  Arbitrary text ending in `/edit` has no editor meaning. Slash command metadata drives completion and dispatch;
  incomplete commands show usage. These rules preserve Leader targeting, drafts and attachment ownership.

- Without `--session` or `--state-root`, TUI/exec selects the nonarchived session most recently opened in the
  canonical launch directory (`--cwd` when supplied), creating a named session if none matches (D-526).
  Explicit roots/IDs and TUI switches retain the selected session's workspace. Session-dependent CLI reads and
  controls select the same recent session without creating one or changing recency; direct daemon startup
  retains its explicit/default root semantics. `sessions new` binds the creation directory without starting work.
- Successful client opens advance a strictly ordered timestamp in `session.workspace.json` under the registry
  lock. History/list/search, failed opens and background work do not. Existing Leader rows override this hint;
  records without an open timestamp use creation order as a deterministic fallback. Archived sessions are
  excluded until explicitly restored/opened. Unusable automatic candidates emit a note and are skipped;
  invalid recency hints cannot block unrelated sessions. Explicit opens report the selected session's errors.
  Fresh workspace and daemon identities are absolute; every cold
  start loads configuration from the saved workspace. Relative legacy Leader paths are refused rather than
  reinterpreted against the reopening directory. Exec acceptance checks retain their explicit client cwd rule.

- One daemon owns each session; the TUI is not a second execution engine. Protocol 2 separates delivered cursors
  from snapshot/log heads. Pages have row/byte budgets, fixed-end drains and explicit retention resync. Ordinary
  refresh never acknowledges unread events; oversized events require explicit resync. Headless completion drains
  through its sampled head. Frames are bounded at 32 MiB; history pages expose an older-page cursor and byte cap.
- The Herdr-inspired TUI has a member sidebar, Leader conversation, responsive inspection, tasks and recent
  activity. It is an independent Rust projection, not an embedded multiplexer or inferred terminal-state runner.
  Selection never redirects composer, interrupt or profile controls. Drafts, attachments and approval ownership
  survive navigation. Separate member history cannot enter Leader context or the shared tree.
- Member history retains at most 400 entries; activity retains 200 observed summaries and is not a complete
  event history. Reads run only for visible content, failed reads retain refresh debt, focus stays visible after
  resize/command receipts, and image metadata/cache retention spans every viewport in a frame. Task outcomes
  remain distinct from process-stop evidence; missing usage and unknown effects remain explicit.
- F6 cycles composer/team/inspection focus; Alt+1 through Alt+6 select Chat/Tasks/Activity/Inspect/Tree/Wall. Enter submits,
  Shift+Enter inserts a newline, Ctrl+C interrupts, Ctrl+Q detaches, Ctrl+K opens the command/goto palette, Ctrl+F searches
  chat, Ctrl+S opens sessions, Ctrl+P pending inputs and Ctrl+M model/effort selection. All slash commands share one
  registry. Completion edits the draft; a separate Enter executes it.
- D-532: the interactive workspace TUI extends Herdr-style multi-pane observation with: (1) explicitly confirmed
  member pause/resume/terminate (`p`/`r`/`X`) and task cancellation (`x`) plus single-shot worker direct messaging (`m`,
  automatically resetting to Leader after submit or on `Esc`); (2) fuzzy `Ctrl+K` command/goto palette and interactive
  `/rewind` / `/mcp` / approval-detail modals alongside direct CLI/subcommand fallbacks; (3) foldable session exploration
  tree (`View::Tree`), multi-member live wall (`View::Wall`), and inspector pinning (`P`); (4) per-tool click toggle,
  `edit_file` diff counts, `Ctrl+F` in-chat search, and OSC 52 code-block copy; (5) single-source-of-truth status
  layout: top bar carries only session/workspace and recorded token/cache/deadline usage, the composer activity row
  carries live runtime state (with elapsed busy seconds) and transient notices, redundant sidebar work summaries and
  duplicated panel/border hints are removed while key hints live in the footer; and
  (6) a bounded read-only background `LiveMonitor` probing sibling daemon sockets for the sidebar live-session strip.
  All lifecycle/task actions require explicit non-repeat confirmation, never alter capability grants or ownership, and
  keep Leader fallback intact.
- Session search/fork/rename/archive/restore/delete preserve authoritative history. Fork needs a stopped source
  and fresh execution evidence. Listening endpoints are protected regardless of greeting; only missing/refused
  sockets establish absence. Other errors retain/refuse. Explicit stop propagates failures and rechecks absence
  after acknowledgement. Startup/check-to-use races and full-backlog connection deadlines remain open.
- Maintenance accepts protocols 1/2 for checkpoint/shutdown only; normal clients require 2. Update health needs
  a valid checkpoint. Version discovery accepts the CLI JSON string or strict legacy label, not path-like values.
  A verified extracted binary can bootstrap a transition when an installed updater cannot handle it.
- Rollback validates the database before switching; binary rollback does not undo migration. Failed startup
  terminates/reaps only its own child, restores the actual starting version and retains target/recovery/journal
  errors. Uncertain liveness never permits socket unlinking; coordinator-owned cleanup remains authoritative.
  `--no-restart` leaves an existing daemon alone. Recovery restores `current` even if another link is obstructed;
  same-release apply does not restore stale `previous`. Failed recovery records `failed`, successful restoration
  `rolled_back`; switch/post-stop journal failures enter the same recovery path.
- Export streams history/revisions/references from one read snapshot, never overwrites a destination and retains
  explicit provenance.

## Repository and verification

- Keep current source, documentation and runnable development/verification tools. Delete superseded reports,
  one-off historical scripts and stopped scratch; Git retains tracked history. Do not remove user state,
  credentials, active jobs or external backups as repository housekeeping.
- Source counts come from Cargo lists. Every registered current total/triple must match; missing/ambiguous
  statements fail. Numeric regeneration does not establish a passing gate, install or formal result.
- Current source contains 926 Rust tests: core 212, engine 566 and TUI 148.
- Formal material has 387 configurations across 47 modules: 63 positive configurations in 60 traversals,
  323 controls and one separate wide configuration; 79 witnesses and 9 Kani harnesses. Shared traversals require
  identical model/specification/domain. Witnesses establish reachability, not safety. The report binds verified
  input paths and bytes with a fingerprint; recomputing it does not replace executing the relevant gates.
- Search-tree coverage includes ordered pairs, 23 controls for every non-`TypeOK` claim and 11 witnesses.
  Executable tests cover Unicode/JSON, SQLite rollback, migration refusal and control walks without displacing
  approval coverage. `TypeOK` is a type claim, not behavioral completeness.
- `make test-aarch64` runs the 27-tool GNU user-emulation subset. Test targets audit leaks even after test
  failure, and either failure fails the target. Emulation is not native ARM/kernel/package acceptance.
- Leak audits bind a run marker to its scratch registry. A marked run attributes processes by the inherited
  marker and directories by its own registry. A missing or malformed binding fails before cleanup. Peer
  registries and unattributed resources are retained. With no marker, a machine-wide audit requires an
  isolated environment. PID start identity and served roots are checked around pidfd acquisition before
  signalling. Ownership coverage is checked with `python3 tools/leak_guard.py --audit-ownership`.
- Full Rust/SQLite refinement, whole-system composition, wide exhaustive completion, Policy B source-bound
  bidirectional fair-trace equivalence and arbitrary-input/live-model quality claims remain unestablished.
- D-533: a command's replay identity is the canonical form of its method, arguments and actor, not
  `serde_json`'s rendering of them. Both crates build `serde_json` with `preserve_order`, so object keys keep the
  wire's order; hashing `to_string()` made two encodings of the same arguments differ and refused a retry as
  "already used with a different payload". Object keys are ordered recursively, array order is preserved as data,
  and the pre-canonicalization hash is still accepted on read so receipts written earlier replay.
- D-534: a stored wait whose `conditions_json` cannot be read, or holds no conditions, never satisfies that wait.
  Registration already refuses an empty list, so such a row is damaged or foreign state, not evidence: `fire_timer`
  leaves it PENDING and `blocked_report` names it with its parse error. `evaluate_wait` initializes an ALL wait's
  accumulator to true, so parsing the row as an empty list used to wake the instance.
- D-535: every command the driver polls with is a read when it changes nothing, and a read keeps no receipt.
  `drain_inbox` was exempted first; `fire_timer` was not, so a WAITING instance wrote one receipt per 100 ms poll
  indefinitely. The exemption is keyed on the command and on its result showing no applied, sealed, woken or
  satisfied fact.
- D-536: a driver that ends in error while its row still says ACTIVE is parked with that error, exactly as a driver
  that cannot boot is. Dropping the finished task's `Result` and respawning on the next pass was a silent hot loop
  that reopened storage every poll. Storage backpressure is retryable in the driver rather than fatal.
- D-537: Worker capability requests (`WAITING_CAPABILITY`) and Worker operation approvals (`WAITING_APPROVAL`)
  route to the session Leader instead of blocking solely on the user (`core/src/v2/control.rs`,
  `engine/src/v2/driver.rs`). Session bootstrap issues `boot-grant-delegable-*` grants (`manage`, `delegate`,
  `message`, `web` at `session` scope and `shell@workspace` when workspace-backed) so in-scope `spawn`/`delegate`
  calls attenuate task-scoped child grants directly. Pending Worker capability requests and non-self-update
  operation approvals queue an envelope to the active Leader and supersede a `WAITING` Leader wait (`work_attention`)
  so the Leader can inspect and decide them via the Leader-only `decide_approval` tool (`list`/`approve`/`deny`).
  Strict bounds apply: a Leader may only approve within its own live covering grants (`issue_task_grant` with
  parent cascade revocation for capabilities; `capability_gap` for operations), may never approve or deny its own
  requests/operations or `request_self_update`, and Workers may never decide approvals. User overrides via `Ctrl+A`
  or the CLI remain unconditionally available.
- D-538: Model-facing native tool schemas use concise single-word names (`read`, `write`, `edit`, `image`, `search`,
  `fetch`, `history`, `checkpoint`, `cancel`, `verify`, `decide`, `tree`) while retaining silent legacy aliases
  across execution, kernel parsing, takeover read-only checks, file rewind SQL, codemode, and transcript
  rendering; TOML `[tools.*] kind`, internal daemon RPC methods, SQLite event kinds, CLI subcommands, and the 21
  slash-command names remain unchanged. Interactive onboarding and UX unify `WAITING_CAPABILITY` capability requests
  into the `Ctrl+A` approval drawer (`approvals` snapshot + `capability_requests`), add `Ctrl+K` work-scope
  recovery actions (`snapshot.work.id`), add plain-English palette keyword search and beginner onboarding prompts
  (`[Menu]`, `[?]`, `[Model]`, clickable starter cards, and `init`/`doctor`/`ensure_daemon` quick-start hints).
