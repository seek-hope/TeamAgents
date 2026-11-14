# TeamAgents design and acceptance baseline

This is the current design baseline. It describes current requirements, architecture and acceptance obligations,
not earlier implementations. Confirmed rules are summarized in [DECISIONS.md](DECISIONS.md); evidence and open
obligations are in [ACCEPTANCE.md](ACCEPTANCE.md).

## 1. Confirmed requirements

| # | Requirement | Current implementation constraint |
|---|---|---|
| Q1 | Task success and long-horizon reliability matter equally at the same model and budget | Measure outcome, wrong completion, recovery, cost and duration separately; turn termination alone is not useful completion |
| Q2 | Language and component boundaries may change | The implementation is Rust-native; test-only fake services and probes may use Python |
| Q3 | Scope may be re-cut; kernel and runtime come first | `core/src/kernel`, `core/src/v2` and `engine/src/v2` are the authoritative execution path |
| Q4 | Context, messages and tool access are isolated by default | Enforce logical permissions; same-user host processes are not confined by that boundary |
| Q5 | The Leader manages instances/connections and may delegate authority | Grants narrow only; revocation cascades; spawned instances have no ambient Shell |
| Q6 | Minimal Leader TUI, with comprehensive management in the developer CLI | One conversation, conditional approvals, distinct interrupt/quit, optional session browser, no topology canvas |
| Q7 | Closing the UI leaves work running and supports reconnect/intervention | Daemon lifetime is independent of the front end |
| Q8 | The user talks to the Leader, may read/address other instances and may pause/cancel | The user has global read authority; agents remain isolated by permissions |
| Q9 | Instances are retained and sessions are isolated by default | Instance, task, work and turn lifecycles are separate |
| Q10 | Work continues by default, with optional budget and bounded failures | No implicit short work timeout; retries, concurrency and resource use remain bounded |
| Q11 | Required checks must pass; other claims carry evidence/unverified status; review is on demand | Failed acceptance enters bounded repair; no extra review instance is required for every task |
| Q12 | Basic file/terminal/web tools, MCP and Skills ship without an external Codex backend | All execution uses the current Rust runtime and explicit bindings |
| Q13 | Multiple providers and mixed models can coexist in one team | DeepSeek is the primary acceptance baseline; every profile uses its native context window |
| Q14 | The workspace is shared by default; isolated directories and Git worktrees are optional | Shared project access is explicit; private conversations are not shared with it |
| Q15 | Authorized work resumes after restart; unknown effects are verified first and never blindly replayed | Persisted location drives recovery; unknown outcomes park with a notification |
| Q16 | Single-instance behavior must not regress and collaboration should show reproducible gain | Controlled, matched comparisons are required; a scripted gain is not a real-model outcome claim |
| Q17 | Estimate usage cost before committing a full multi-round budget | Record usage and uncertainty honestly; do not infer priced cost from estimates alone |
| Q18 | Old config/session compatibility is not inherently required; deliberate migration and cleanup are allowed | Current migration is explicit and refuses foreign or unexpected state |
| Q19 | Cleanup follows an explicit ownership decision while Git history remains | Current source and tools are kept in the tree; old state is not silently adopted |

The implementation targets one machine and one system user. Distributed execution, multi-tenancy, an external
execution backend and a plugin marketplace are outside the current design.

### 1.1 Current delivery scope

The current delivery includes the Rust kernel, durable runtime, daemon, CLI, TUI and current development tools.
Distributed execution, multi-tenancy and an external execution backend remain outside this scope.

## 2. Overall design

```text
User / TUI / exec / CLI
          |
          v
Unix-socket JSON-lines daemon
          |
          v
Supervisor and per-instance drivers
          |
          v
Bounded storage worker -> Control::submit -> SQLite (WAL + synchronous=FULL)
          |
          +--> phase machine -> model provider
          +--> durable operations -> tools / Shell runner / MCP
          +--> context views -> summaries + session exploration tree
```

- **Control plane:** `core/src/v2/control.rs::Control::submit` performs ingest, validation, reduction and
  persistence in one transaction. Identity and revisions are assigned there.
- **Store:** `core/src/v2/store.rs` checks the format stamp, applies WAL and `synchronous=FULL`, and migrates
  only known versions explicitly. Schema 9 is current.
- **Execution:** `engine/src/v2/driver.rs` owns the phase machine; `engine/src/v2/supervisor.rs` coordinates all
  ACTIVE instances. Model and tool waits are outside transactions.
- **Products:** `engine/src/v2/daemon.rs`, `engine/src/v2/exec.rs`, `engine/src/cli.rs` and
  `tui/src/daemon_client.rs` expose the same runtime. No second engine runs inside the TUI.
- **Effects:** Shell runs through `engine/src/jobs`; file/web/Skill tools are in `engine/src/tools.rs` and
  `engine/src/search.rs`; MCP is in `engine/src/mcp.rs`; bindings in `engine/src/bound.rs` are authorization.
- **Presentation:** TUI rendering and hit-testing share `tui/src/v2ui.rs::geometry`.
- **Development tools:** current executable checks live under `tools/`; there is no review/evaluation data
  dependency in the current tree.

## 4. Storage, authoritative state and atomic boundaries

### 4.1 One database per session

The store includes instances, work scopes/states/inputs/requests, tasks, grants, envelopes and input revisions,
context entries, model requests, attempts, decisions, operations, approvals, waits, artifacts, events, command
receipts and session-tree revisions/head. SQL/JSON projections are the runtime representation.

### 4.2 Facts that must commit in one transaction

Durability follows this order:

1. Stage large bytes and verify identity/digest before referencing them.
2. Begin one immediate transaction and re-check replay identity and current guards.
3. Persist business facts and outbox events atomically.
4. Publish verified artifacts and receipts with explicit state.
5. Recover from the recorded location; never infer a known outcome from an uncertain side effect.

### 4.3 Large context and write ordering

Large outputs and complete responses become immutable artifacts, while small structured receipts stay in SQLite.
An artifact is staged, verified and published before its reference commits. Compaction changes only the model
view: original entries remain in history and can be read back.

### 4.4 Durability strength and versions

A foreign or wrong-version database is refused. Migration preserves existing work and history, derives the
Leader role, and rejects unexpected pre-existing tree tables without changing the original store. Binary rollback
alone does not undo a schema migration.

## 5. Identity, capability, communication and collaboration

### 5.1 A single authorization basis

`CapabilityGrant(subject, action, resource, parent, revision)` is the authorization basis. A grant can only
narrow its parent; revocation removes descendants. Creating an instance grants neither arbitrary connections
nor blanket file permissions. Binding an MCP service or web tool does not bypass grants.

### 5.2 The model-visible surface

Tool and collaboration schemas are selected by current grants, profile bindings and model capabilities. The
surface shown to a model is not proof that the model used it correctly.

### 5.3 Delivery, wakeups and revocation

Communication is queued and applied at the recipient's safe boundary. Visibility and push delivery are distinct;
observer payloads are scoped. A user may read every instance, while an instance reads only its own private
history. The session tree is built only from Leader-visible inputs and already shared results, never from
member-private history.

### 5.4 Lifecycle and user intervention

Workspace policy is selected by `spawn.workspace`. Shared, isolated and Git-worktree policies are resolved before
the child starts and recorded in `workspace.json`. Retirement refuses dirty or unmerged work. Task cancellation
alone is not process-stop evidence; shared-workspace takeover separately observes the recorded local process
 group without claiming global/remote quiescence.

## 6. Side effects, tool processes and recovery

### 6.1 Pre-execution protocol

Dispatch is the authorization linearization point: operation identity, argument hash, grant revision and any
approval are rechecked before an effect starts. A stale schema never authorizes an operation.

### 6.2 The necessary scope of the shell runner

Requests use the profile's declared native window. Capacity includes instructions, schemas, history, output and
protocol reserves. Provider-reported usage supersedes estimates. Unknown usage stays unknown. Logical request ids
and transport attempt ids are separate, and only a selected complete response advances the turn.

Compaction follows actual occupancy. The summary keeps the original request, revisions, acceptance criteria,
open questions and receipt references. Covered entries are hidden from the model view, not deleted from storage.
The retained tail remains the current working set.

The Leader's compact response can contain a structured summary and an exploration-tree patch. A legacy text
summary may still commit without a tree update. Invalid or stale tree patches emit an explicit failure and leave
the old tree stale; they do not fail the summary or the user's turn.

### 7.1 Session exploration tree

There is one exploration tree per session. Nodes classify questions, hypotheses, attempts, findings, decisions,
obstacles and next steps, and carry source references and a focus. The tree is derived model navigation, not
verified fact, authority, completion evidence or an alternate execution checkpoint.

Only the persisted Leader writes it. Leader compaction advances the previous cumulative tree through newly
visible Leader entries, including the retained recent turn. Subsequent Leader prompts receive a bounded
focus/unresolved projection. `/tree`, `teamagents search_tree [--json]`, daemon reads and export share one
projection.

Patches use the ordinary durable operation ledger and a base revision. Add/update/move/merge/focus are atomic;
ordered operations validate against the preceding result and roll back as a unit. Merge unions references,
reparents children and transfers focus correctly. Fresh patches create at most 96 nodes, 32 operations and 32
levels. Exact replay returns the prior result without growth and reconstructs assigned `added_ids` when needed.
Omitted nodes and old revisions remain inspectable; no operation deletes conversation history or rewinds effects.

## 7. Tools, integrations and providers

File tools are workspace-confined. Shell, acceptance commands and local MCP run on the host with explicit
receipts, process-group cancellation and durable journals; this is not an OS sandbox. Provider credentials are
not automatically inherited by tool processes.

### 6.3 Recovery matrix

After interruption, persisted phase and operation location decide reuse, failure or `OUTCOME_UNKNOWN`. A known
result is consumed once; an uncertain external effect is not replayed. Late results remain in their original
epoch and cannot take over new work.

### 6.4 Pause, cancellation and shutdown

Interrupt cancels the in-flight model request, while running tools and delegated work keep their lifecycle.
Cancellation persists intent before any signal and does not prove execution stopped. Instance termination is
user-only, and TUI detachment does not stop the daemon.

## 7. Models, tools and context

MCP supports stdio and streamable HTTP. `codemode` composes bound tools in a restricted JavaScript sandbox with
no ambient filesystem, network or timers. Resource list/read uses the existing service binding and requires an
explicit service when multiple bindings are ambiguous. OAuth input and credential writes are bounded and atomic.

Skills are read on demand and never grant authority. User hooks may veto native calls or emit notifications.
Project configuration cannot silently supply permissions, hooks, checks, retention or update policy.

## 8. Work, budget and completion

Work scopes own budget, deadline, admission and acceptance. Work states/tasks own continuation and attribution.
Every model attempt, retry, compaction, review and verification shares the same work budget. Reservations happen
before requests; actual or unknown usage is settled honestly.

Enrolled work can continue after progress prose, with bounded recovery for empty/plan-only/repeated loops. A
blocked task does not satisfy a successful task-result wait. User input can supersede a wait without settling
its condition. New input cannot replenish an active ceiling or resume parked work.

Completion requires the actual deliverable and required checks. Failure, blocking and cancellation remain honest
outcomes. Check evidence binds inputs and observation time. Independent review may be requested, but repeated
agreement from one model is not objective proof. Reviewer calls have their own reservation, settlement and
interruption recovery; automatic allow is limited to a matching low-risk, high-authorization operation and
never self-update.

## 9. Protocol and interface

The daemon speaks JSON lines over a per-user Unix socket. Requests carry `protocol_version`, `request_id`,
`method`, `params`, and business `command_id`. Events carry session, sequence, kind and visible payload reference.
A reconnect reads a consistent checkpoint/watermark before consuming newer events. Streaming previews are
transient and never authoritative facts.

The TUI has one Leader conversation, elevated prompts, grouped operations, quiet model chrome, Markdown and
media rendering. Approval input ownership is preserved. `/tree` shows the session exploration tree; natural
language requests let the Leader reorganize it. CLI and JSON export are read-only projections, not second
business stores.

## 10. Module organisation and dependencies

`tools/` contains executable development and verification utilities, generated-document checkers and current
product probes. `tools/tmp/` is ignored scratch for new local runs, not source or committed evidence.

The current local gate has 777 Rust tests. Formal material has 393 configurations across 48 modules: 65 positive
configurations, 327 controls and one separate wide configuration, evaluated in 62 positive traversals. There
are 79 normal-path witnesses and 9 Kani harnesses. Witnesses demonstrate reachability only.

## 11. Current acceptance boundaries

Local scripted tests do not establish real-model quality, priced cost, external-provider/OAuth acceptance,
clean-host installation, a complete graphical-terminal matrix or general long-horizon reliability. Finite models
do not establish full Rust/SQLite refinement, whole-system composition, wide exhaustive completion or Policy B
source-bound equivalence.

## 12. Acceptance matrix

| ID | Scenario | Current acceptance constraint |
|---|---|---|
| A01 | A single Leader completes work, including team-built work | Verify actual outcome and delegation, not prose alone |
| A02 | A→B→C→A communication | Authorized ordered message application |
| A03 | Limited delegation and parent revocation | Only narrowed grants and cascading revocation |
| A04 | A queued action meets revocation | Dispatch rechecks the current permission revision |
| A05 | Reading another instance's history | User reads all; an instance reads only itself |
| A06 | A message is applied across restart | Envelope identity deduplicates application |
| A07 | Permanent start failure | Classify setup failure without executing or pretending success |
| A08 | Crash after tool success before result consumption | Reuse the persisted result; do not repeat the effect |
| A09 | Unknown external outcome | Record `OUTCOME_UNKNOWN` and park rather than guessing |
| A10 | Duplicate dispatch / duplicate GO | Operation identity and durable receipts suppress duplicates |
| A11 | Daemon and runner crash separately | Recover each from its own persisted journal |
| A12 | A Shell service outlives command exit | Retire settled runners without losing their journals |
| A13 | Cancel / timeout / completion races | Persist cancellation first and record actual process state |
| A14 | Host execution and retired configuration | Host execution is explicit; retired sandbox configuration is refused |
| A15 | Environment and process identity | Record boot, cwd, executable, process group and observation limits |
| A16 | A required check fails | Repair within bounds or block; never claim success |
| A17 | Artifacts change after a check | Revalidate declared inputs before completion |
| A18 | Multi-instance usage budget | One shared ceiling with reservations and known/unknown settlement |
| A19 | Truncated stream and connection loss | Preserve framing/retry boundaries and honest unknown outcomes |
| A20 | Context compaction, original-history preservation and restart | Keep references/originals and update the session tree without blocking summary on tree failure |
| A21 | The user adjusts an instance directly | User-only intervention preserves ownership and lifecycle rules |
| A22 | Wait/deadlock diagnosis with ALL/ANY conditions and external paths | Report concrete conditions without claiming arbitrary graph-deadlock proof |
| A23 | Deferred facts, wait registration, sweeps and wakeup races | Register and satisfy waits atomically with the triggering fact |
| A24 | Late receipts after reset/termination or epoch change | Keep late results in their original epoch and prevent stale takeover |
| A25 | MCP over stdio and streamable HTTP | Use bound authority and consistent receipts; ambiguous resources fail closed |
| A26 | Skills permissions | Skill content cannot grant Shell or other authority |
| A27 | Heterogeneous providers and mixed models | Route each member through its own adapter/profile; fakes do not prove cooperation |
| A28 | Disconnect, slow client, reconnect and watermark recovery | Resume from a consistent snapshot and never treat previews as committed facts |
| A29 | Session isolation with shared project/workspace options | Separate file-sharing policy from private-history visibility |
| A30 | Artifact persistence, publication, release and GC | Stage, verify, publish, reference and collect in explicit states |
| A31 | Disk full / persistence-write failure | Stop new side effects and report unpersisted in-flight work honestly |
| A32 | Large-history, storage, rendering and slow-reader behavior | Keep bounded views and avoid redundant copies without weakening durability |
| A33 | Second daemon, coordinator exclusion and lock recovery | One coordinator per state root with explicit recovery |
| A34 | Foreign or wrong-version state-store refusal | Refuse foreign state; migrate known versions atomically and reject unexpected shapes |
| A35 | Work deadline | Past deadline no new request starts; preserve an explicit recovery path |
| A36 | Store/session lifecycle, search/export and guarded cleanup | Read consistently, export without replacement, migrate and clean only by explicit scope |

## 13. Performance experiments and success criteria

### 13.1 Local cost and model success rate are separate checks

Latency, storage growth, allocations and model outcome are different measurements. A faster probe does not prove
better task success, and a green model reply does not prove efficient execution. Use the production
`engine/examples/load_probe.rs` and real task evidence for each claim.

### 13.2 Samples, budget and integrity

Performance fixtures are synthetic unless explicitly identified otherwise. Record machine conditions, input shape,
actual command and exit status. Real-model trials use the declared native context window and cannot be replaced
by a smaller-window approximation.

### 13.3 No thresholds without evidence

Do not introduce a performance threshold merely to claim improvement. A timeout, stale fixture or passing root
without the intended semantic obligation is not evidence. Keep the existing large-history, lazy-revision and
viewport checks; no new full performance figure is asserted here.

## 16. What still needs evidence, and the definition of done

A change is locally complete only when its actual acceptance checks pass, relevant documents are synchronized,
and remaining external obligations are named. A green Cargo run, simulation, witness or plan alone does not prove
real-service acceptance. A timeout, skipped proof or unsupported model is not rewritten as a pass.
