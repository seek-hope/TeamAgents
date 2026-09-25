# Formal verification (control plane and beyond)

Goal: turn the **confirmed protocol properties** of the [design](../docs/DESIGN.md) into machine-checkable
specs instead of relying on sample tests alone.

Verification has three layers, all material sitting in this directory: (1) **protocol models** — TLA+/TLC
specs (`tla/V2*.tla` plus `MC*.cfg`) abstracting the protocol behaviour of `core/src/v2/control.rs` and
`engine/src/v2/driver.rs`; (2) the **executable spec-to-code correspondence** — `core/tests/v2_invariants.rs`
recomputes the invariants over real command sequences; (3) the **pure-function layer** — bounded enumeration in
`core/tests/kernel_properties.rs` plus Kani proofs in `kani/`. The TLA+ model is **not a refinement proof**:
results on the model do not automatically hold for the code, and code-side results come from bounded
exploration. The boundaries are in "Boundaries" below and in [REPORT.md](REPORT.md).

## Running

```bash
make verify-model           # small control-plane configuration (seconds)
make verify-model-all       # small configurations for all seven modules (control plane, artifacts, waits,
                            # tasks, compression, daemon, required checks)
make verify-model-wide      # wide control-plane configuration (2 instances / 2 operations; hundreds of millions
                            # of states, slow)
make verify-kani            # paging arithmetic (needs the Kani toolchain, see below)
cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants   # spec-to-code correspondence
```

The first run downloads the pinned `tla2tools.jar` (TLC v1.7.1, SHA-256 in the Makefile) into
`$TLA_TOOLS_DIR` (default `~/.local/share/teamagents-verify`) and verifies it; the toolchain never enters the
repository and never enters `make check`. Java is required (this machine uses OpenJDK 27).

## Specs and configurations

| File | Contents |
|---|---|
| `tla/V2Control.tla` | control-plane abstraction: instance phase machine, requests/attempts, decisions and operations, approvals, the dispatch linearization point, cancel/timeout, epoch resets, goal budget reservations and settlement, crash/recovery |
| `tla/MC.cfg` | small configuration (1 instance / 1 operation / 2 request slots / 1 attempt slot / 1 epoch reset / 1 unknown usage) |
| `tla/MC_wide.cfg` | wide control-plane configuration (2 instances / 2 operations with one requiring approval / 3 request slots / 2 attempt slots) |
| `tla/V2Artifact.tla` + `tla/MC_artifact.cfg` | artifacts and GC: write bytes → STAGING row → reference and LIVE in one transaction → GC claim → delete/abandon |
| `tla/V2Wait.tla` + `tla/MC_wait.cfg` | waits/wakeups/timers/supersede: evaluate at registration → parked drain scan → answer in the same transaction when satisfied → cancel/supersede/re-arm |
| `tla/V2Task.tla` + `tla/MC_task.cfg` | task lifecycle and goal settlement: delegation (dependencies must exist first, the goal must be ACTIVE) → start → settle/cancel → system parking → termination cascade; goal creation, request admission, open operations, settlement and detach |
| `tla/V2Compress.tla` + `tla/MC_compress.cfg` | context compression (A20): open/submit/fail/cancelled by a closed epoch; summaries append at the tail, coverage only grows and originals are never deleted |
| `tla/V2Daemon.tla` + `tla/MC_daemon.cfg` | session daemon protocol (A28): deduplication and replay of stable command ids, the atomic snapshot+watermark pair of `checkpoint`, gap-free `events(since)`, a slow client never blocking the writer |
| `tla/V2Checks.tla` + `tla/MC_checks.cfg` | required checks (A16/§8): only self-reported successes are verified, failures enter a bounded repair round, an exhausted budget or an unusable verification path (stale observation, refused dispatch) parks the goal BLOCKED, and a candidate is never upgraded |

The environment (tool results, approval timing, crash points) is **non-deterministic** in the model; that is
exactly what is enumerated.

## Verified properties and their code anchors

| Property (spec) | Meaning | Code anchor | Acceptance |
|---|---|---|---|
| `TypeOK` | phase/lifecycle/operation status/effect counters hold legal values | the `models.rs` enums, `OpStatuses` | §4.1 |
| `NoEffectBeforeApproval` | an operation needing approval has no effect before it is approved | the approval gate in `dispatch_operation`; `approve`/`deny` | A25/A12 |
| `RecordBeforeEffect` | a persisted dispatch record exists before any effect | `dispatch_operation` writes `DISPATCH_COMMITTED` first | A08/A11 |
| `EffectAtMostOnce` | an operation has at most one external effect (recovery never replays) | the recovery path only sets `OUTCOME_UNKNOWN` | A08/A10/A13 |
| `ReservationsAdmitted` | live reservations never exceed the ceiling (a consequence of the admission gate) | `known+reserved+est ≤ max` in `reserve_budget` | A18/§8 |
| `AdmissionGate` (temporal) | every entry into `MODEL_PENDING` passed the admission gate | as above | A18/§8 |
| `ReservationReleased` | closing a request (complete/fail/cancel) always releases its reservation | the `release_reservation` call sites | §8 |
| `OneActiveRequest` | an instance has a single active request at a time | the phase/revision guard in `begin_request` | §3/§6.1 |
| `SelectionIsComplete` | only an atomically selected complete attempt exists | the `selected_attempt_id IS NULL` update in `record_attempt` | A19 |
| `NoTurnWithoutWork` | no new turn opens when the last entry is the model's own text | the closing entry plus the idle test in `step_ready` | §5.4 |
| `StaleExecutorRejected` | an executing instance holds the current revision | `revision == expected` in `begin_request` | §6.1 |
| `NoEffectOnTerminated` | a terminated instance produces no effect | TERMINATED in `set_lifecycle` plus the dispatch guard | §6.4 |
| `PreparedIsNotTerminal` | a `PREPARED` operation has no effect yet | the operation state machine | §6.1 |
| `CancelledBeforeStartHasNoEffect` | "cancelled before start" means no effect (a dispatch record may exist) | `cancel_operation` on `DISPATCH_COMMITTED` | A13 |
| `TerminalOpStable` (temporal) | a terminal operation is never rewritten | the "already terminal" refusal in `complete_operation` | A13 |
| `TerminalGoalStatusStable` (temporal) | a terminal goal is never rewritten | the `already_closed` branch of `complete_goal`/`block_goal` | §8 |
| `NoReceiptAcrossEpochs` (temporal) | a receipt never lands across epochs | `reset_instance` closes the old epoch and cancels in-flight operations | A24 |

### Artifacts and GC (A30)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `NoReferenceToUnpersisted` | a referenced artifact has its bytes persisted | `store_response_artifact` (tmp → fsync → rename, then `artifact_stage`) |
| `LiveIsPersisted` | a LIVE artifact has bytes | `artifact_publish` in the same transaction as the reference |
| `GcClaimsOnlyUnreferencedLive` | a claimed (DELETING) artifact has no references | the candidate condition in `artifact_gc_claim` |
| `CollectorSkipsIncomplete` | no half-written files are left to be collected (STAGING/ABANDONED are untouched by the deleter) | GC claims LIVE only; `artifact_abandon` only marks |
| `ReferencesOnlyLive` (temporal) | a reference attaches to a LIVE artifact, or attaches in the same step as the LIVE flip, and the bytes already exist | `publish_one` plus the reference in one transaction |
| `BytesOnlyDeletedWhileDeleting` (temporal) | a file disappears only while DELETING | the GC deletion order |
| `ClaimOnlyFromLive` (temporal) | GC claims from LIVE only | as above |
| `LiveFlipCarriesReference` (temporal) | STAGING → LIVE always carries the first reference (no "alive but unreferenced" window can be collected) | `publish_list` commits it in the same command |

### Waits, wakeups and timers (A22/A23, RT-06)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `TypeOK` | phase/wait status/answer counts hold legal values | `waits.status`, `instances.phase` |
| `WakeAnswerAtMostOnce` | a wakeup appends at most one answer per wait | the `PENDING → SATISFIED` guard in `wake_satisfied_at` plus `append_context` deduplication |
| `AnswerImpliesConditions` | no spurious wakeups: an answer is appended only when the conditions really hold | `evaluate_wait` tests `satisfied` before appending |
| `AnswerImpliesSatisfied` | an answer always appears in the same step as `SATISFIED` | as above (one transaction) |
| `WakeAnswersItsCall` | the answer lands on the wait's own tool_call | `wait_call_id` plus `Observation::ToolResult` |
| `WaitingHasPendingWait` | a parked instance has a PENDING wait of its own | `import_response` sets `WAITING` only when unsatisfied |
| `PendingImpliesParked` | the owner of a PENDING wait is `WAITING` (so the drain is always available) | `submit_input`/`close_epoch_execution` set `READY` only after cancelling the wait |
| `UnusedSlotHasNoAnswer` | an unused wait slot has no answer | wait rows are created per decision |
| `NoStrandedPending` (temporal) | a PENDING wait whose conditions hold and that was not terminated is eventually closed (satisfied by the drain or cancelled by a supersede); it never hangs forever | the parked drain (weak fairness: the driver's poll) plus the supersede path |

### Tasks, delegation and goal settlement (A02/A09/A16)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `TypeOK` | task/goal/operation/lifecycle values are legal | `tasks.status`, `goals.status`, the operation status set |
| `SystemOnlyParksTasks` | the system never settles or cancels a task; its only task write is parking as BLOCKED | `complete_task`/`cancel_task` refuse `Identity::System`; `park_tasks_for_unknown` |
| `OnlyPartiesWriteTasks` | only the assignee, the delegator, the user or the system (parking) may change a task | the identity guards of the four actions; the delegator is the requester |
| `SettledIsFinal` | a terminal task is never rewritten (`SUCCEEDED/FAILED/CANCELLED` land once) | the terminal branches of `complete_task`/`cancel_task` |
| `ReturnPathOnlyWhileOpen` | the narrow return path exists only while the task is unsettled; `SUCCEEDED/FAILED` and cancellation revoke it, `BLOCKED` keeps it | the `revoke_grant_tree` call sites; `terminal = SUCCEEDED|FAILED` |
| `DependenciesPointBackwards` | dependency edges point only at earlier tasks, so the dependency graph is **acyclic by construction** | `delegate_task` requires the dependency to exist |
| `NoSelfDependency` | a task never depends on itself | the self-dependency check in `delegate_task` |
| `NoOpenTaskOnDeadAssignee` | a terminated instance holds no open task | the termination cascade in `set_lifecycle` |
| `NoStaleActiveGoal` | no instance points at a settled goal (settlement detaches the pointer and billing accepts ACTIVE goals only) | `detach_goal` and the `goal_is_active` filter in `budget_goal` (V-G1 fix) |
| `RegisteredWorkNeedsAnActiveGoal` | delegation lands on ACTIVE goals only (monitoring variable `lateTask`) | the `goal_is_active` guard in `delegate_task` (V-G1 fix) |
| `RequestsResolveToActiveGoals` | a request resolves to an ACTIVE goal (monitoring variable `lateRequest`) | the `goal_is_active` filter in `budget_goal` (V-G1 fix) |

The task module asserts safety only: whether a task advances depends on the environment (member turns), and the
design does not require the system to settle tasks on the user's behalf, so no liveness property is written.

## Three findings from the modelling work

1. **The budget property must be written as an admission gate plus a reservation ceiling**, never as "actual
   usage never exceeds the limit": in the model `known` is settled from provider-reported usage and bypasses
   the gate, while `BeginRequest` is the gate. This matches §8 of the design ("no false promise of never
   exceeding when provider billing is incomplete"); a naive formulation is refuted by TLC immediately.
2. **A settled goal keeps receiving billing**: TLC refuted "a goal's records stop changing after settlement",
   and reading the code confirmed that neither `reserve_budget` nor `settle_usage` looks at the goal status
   while `begin_request` resolves the goal through `active_goal_id` — so a new turn after a goal completed
   still billed that `SUCCEEDED` goal. The budget gate still worked (nothing over-reserved), but "goal status"
   and "later usage" disagreed. **That is a product decision** (whether new turns must create a new goal), so
   the model keeps the behaviour and asserts only "terminal status is never rewritten".
3. **`CANCELLED_BEFORE_START` means "no effect happened"**, not "never dispatched": cancelling an operation
   that was dispatched but not started produces exactly that status, so the invariant must constrain
   `effect = 0`.

### Finding V-W1 (fixed, 2026-09-24)

**A resolved wait must answer its own `wait` tool_call — two paths did not.**

Fix: `answer_closed_waits` extends the answer to the two non-drain exits — satisfied at registration (inside
`import_response`, sharing `wait_reason` with the drain) and superseded/closed epoch (`submit_input`,
`close_epoch_execution`). The deduplication key stays the wait id, so a replay never appends a second answer.
The spec side is guarded by `ResolvedWaitIsAnswered` and the regression is
`wait_call_answered_outside_the_drain_path` (both paths assert the answer exists, is appended once and carries
`superseded` for the supersede case).

The counterexamples and probes recorded before the fix:

Strict wire endpoints (OpenAI-style Responses, Anthropic) reject an assistant `tool_calls` message without
matching tool responses, as the repository's own comments state (`core/src/kernel/mod.rs` and the
`wake_satisfied_at` comment). Only the **drain path** (`wake_satisfied_at` scanning `PENDING` waits and
appending an answer) paired them; the other two paths did not answer:

1. **satisfied at registration** (the branch in `import_response` where `evaluate_wait` judges the wait
   satisfied immediately — the very branch A23 uses so a wakeup is not lost): the wait lands as `SATISFIED`
   and the instance never parks, and nothing later appends a tool_result for it;
2. **superseded** (`submit_input` cancels the instance's whole `PENDING` batch, `close_epoch_execution`
   likewise): no answer is appended either, so the instance starts its next turn carrying an unanswered `wait`
   call.

Evidence:

- Spec counterexample (before the fix; the temporary configuration that triggered it was never committed):
  TLC reported the invariant violation (now named `ResolvedWaitIsAnswered`) with the trace `ArmWait(PENDING)`
  → `Supersede` → `CANCELLED` and `answers = 0`; the satisfied-at-registration case was triggered by the
  satisfied branch of `ArmWait` in the same way (`answers` stayed 0). After the fix `V2Wait.tla` writes the
  answer on both paths and `make verify-model-all` is green.
- Code probe (`cargo test --offline --manifest-path core/Cargo.toml --lib
  wait_call_answer_gap_outside_the_drain_path -- --nocapture`):

  ```text
  PROBE A: satisfied=true phase="READY" answers_for_wait_1=0   # satisfied at registration, no answer
  PROBE B: phase_after_import="WAITING" wait_state=CANCELLED phase_after_input="READY" answers_for_wait_2=0  # superseded, no answer
  ```

Impact before the fix: on a strict endpoint the next request on those two paths would be rejected; a tolerant
endpoint (the DeepSeek chat-completions used for local evaluation) accepts it, which is why real runs never
exposed it. The fix extends the answer from the drain path to both, as the user's "fix everything" confirmed.

### Finding V-G1 (fixed, 2026-09-24)

**A goal in a terminal state no longer accepts new work.** Before the fix, three paths ignored the goal
status:

- `delegate_task` checked only that the assignee and (for instance delegation) the delegator were live and
  that `goal_id` existed — never that the goal was ACTIVE;
- when `import_response` opened an operation, `goal_id` came from `request_goal(active_goal_id)` and likewise
  ignored the goal status;
- `reserve_budget`/`settle_usage` ignored it as well (this is finding 2 above).

Spec counterexamples (before the fix; both properties are now guarded by `MC_task.cfg` and TLC is green):

```text
Error: Invariant RegisteredWorkNeedsAnActiveGoal is violated.     # a task was delegated before its goal existed
Error: Invariant ClosedGoalTakesNoNewOperation is violated.       # a settled goal still opened an operation
```

Fix (landed after the user confirmed "fix everything"):

- `budget_goal` accepts only **ACTIVE** goals as billing targets (both the instance's `active_goal_id` and the
  goal of its oldest open task); when neither resolves it runs as "no goal", exactly like a session without
  one;
- `complete_goal`/`block_goal` detach every instance pointer to the goal when they settle it (`detach_goal`,
  returning a `detached` count);
- `delegate_task` requires an ACTIVE goal and otherwise fails with a pointer to `create_goal` first;
- the spec side is guarded by `NoStaleActiveGoal`, `RegisteredWorkNeedsAnActiveGoal` and
  `RequestsResolveToActiveGoals`, with the regression `a_settled_goal_takes_no_new_work` (pointer detached,
  delegation refused, new requests not billed, a fresh goal restoring both, and settled records frozen).

Modelling conclusion (recorded in the `V2Task.tla` header): the linearization point for **new work is the
request (`begin_request`), not the operation**. A request admitted while the goal was ACTIVE may still open
operations and settle usage against that goal after settlement — that is honest accounting, not new work —
which is why the property is written at the request level (`RequestsResolveToActiveGoals`) rather than the
operation level.

Known boundary (not enforced, see the `V2Task.tla` header): `complete_goal` checks open operations but not
tasks, so a goal can settle while its own tasks are still open; those tasks keep running and their later
requests have no billing goal. Tightening that (tasks must settle first) needs a committed "completion
refused" result for the driver and is future work.

### Two more findings about the properties themselves

4. **An in-transaction flip must be written into the property**: an artifact's first reference attaches in the
   same step as `STAGING → LIVE`, so the naive "a reference attaches only to a LIVE artifact" is refuted by
   TLC immediately; the correct formulation allows `row' = LIVE`.
5. **Reference counting cannot use unbounded integers**: `refs++` makes the state space diverge (measured: not
   converged at 180M states), while a **finite owner set** (an `Owners` constant) leaves the same
   configuration with 64 reachable states. The same applies to the later modules.

### Context compression (A20)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `TailAppend` | entries occupy a slot prefix: a new entry appends at the tail and is never inserted in the middle | `MAX(idx)+1` in `append_entry` |
| `NoEntryIsEverLost` | originals are never deleted (coverage is a view fact; the monitoring variable `lost` stays empty) | `compress_context` only writes `compressed_by` and never deletes |
| `CoveragePointsForward` | a summary is always newer than the entries it covers | the summary is appended (at the tail) before coverage is marked |
| `CoverageNeverLifted` | coverage only grows and is never re-pointed at another summary (monitoring variable `uncovered` stays empty) | the coverage statement carries a `compressed_by IS NULL` guard |
| `NewestSummaryIsVisible` | the newest summary is never covered itself (earlier summaries may be covered by later ones) | the commit order |
| `CoveredStaysCoveredByItsSummary` | a covered entry points at a later, real summary | as above |
| `ClosedCompressionReleasesReservation` | closing a compression request (complete/fail/cancelled by a closed epoch) releases its reservation | `release_reservation` in `compress_context`/`fail_compression`/`close_epoch_execution` |

The admission of a compression request (lifecycle, goal deadline, budget gate) uses the same code path as a
turn request and is covered by `V2Control`'s `AdmissionGate`, so it is not modelled again here.

### Session daemon protocol (A28)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `LogMonotone` | the event log only grows: versions are never reused or rolled back (monitoring variable `shrank` stays FALSE) | the auto-incrementing `events.sequence`; `read_events(since)` |
| `AppliedAtMostOnce` | a command id takes effect at most once | the `commands` table deduplication in `submit_inner` |
| `ReceiptsAreStable` | a stored receipt is never rewritten; a replay with the same payload returns the **stored** receipt (monitoring variable `drift` stays empty) | `submit_inner` returns `result_json` unchanged when the command id exists |
| `ReceiptNamesARealVersion` | the version a receipt names really exists | as above |
| `AppliedCommandsUsedTheWireVersion` | only handshake-compatible protocol versions may submit commands | the `PROTOCOL_VERSION` check |
| `SnapshotNeverLeadsCursor` | a snapshot never claims a version ahead of the client's watermark — exactly what "snapshot plus watermark in one read transaction" buys | `checkpoint` in `daemon.rs` (reads the snapshot and `MAX(sequence)` inside one `unchecked_transaction`) |
| `ViewMatchesCursor` / `CursorNeverBeyondLog` | after a reconnect the view and cursor agree with no gaps | `events` returns every event with `sequence > since` |
| `NoResyncInThisVersion` | this version never reclaims events, so `resync_required` is always false (`pruned` is never set) | the header note "Events are never reclaimed in this first version" |

`SnapshotNeverLeadsCursor` is a **non-vacuous** property: splitting `checkpoint` into "write the snapshot, then
the watermark" (i.e. not one read transaction) is refuted by TLC immediately (measured:
`Error: Invariant SnapshotNeverLeadsCursor is violated`). The "a slow client never blocks the writer" part is
structural: `RuntimeEvent` does not depend on any client cursor, so no liveness property is written here.

### Required checks (A16/§8)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `SuccessRequiresAllChecksPassed` | a goal becomes SUCCEEDED only in a round where **every required check really passed** (the property is written on the **observed result**, not on a "verdict" variable) | `step_completion_checks`: `complete_goal` only when `failures.is_empty()` |
| `NoUpgradeOfTheCandidate` | the runtime never upgrades a model candidate: an admitted non-delivery is never SUCCEEDED (monitoring variable `nonSuccessSuccess`) | only self-reported successes are verified, and `complete_goal` settles the **stored** candidate |
| `ChecksOnlyVerifyAClaimedSuccess` | a candidate that does not run the checks settles by its own outcome (monitoring variable `lateRound`) | `step_completion_checks` only runs when `outcome == "success"` |
| `RoundsAreMonotone` / `RoundsAreBounded` | rounds only grow and stay within budget (monitoring variable `rewound`) | the `rounds >= max_rounds` branch |
| `BlockedAfterTheBudgetOrStale` | a self-reported success lands BLOCKED only for "budget exhausted" or "verification path unusable (stale observation)" | the `infra` (`dispatch_refused`/`spawn`) and `stale_inputs` classifications; `block_goal` |
| `NoUnverifiedSuccess` | no success is unverified (monitoring variable `upgrades`) | as above |

Non-vacuity evidence: relaxing `Accept` to "one pass is enough (even with a failure)" is refuted by TLC
immediately (`SuccessRequiresAllChecksPassed is violated`); writing the property as "SUCCEEDED implies the
recorded verdict is pass" would be **vacuous** (the action writes that variable itself), which is why the final
assertion binds the observed result.

## Executable spec-to-code correspondence (`core/tests/v2_invariants.rs`)

The specs check an abstract state machine. `core/tests/v2_invariants.rs` (part of `make check`) recomputes the
same invariants against the real `core::v2::Control`:

```bash
cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants
```

- **Enumeration**: every command sequence of length ≤ 2 starts from a fresh database (38 command kinds, so
  1,482 sequences), including refused combinations;
- **Random walks**: 60 fixed-seed walks of 24 steps, each step choosing only among commands usable right now
  and preferring the kind used least in this walk (coverage driven; otherwise the walk repeats one safe action
  and never reaches deep paths). Fixed seeds make every trace reproducible;
- **Re-checked after every step**: `TypeOK`, `SettledIsFinal`, `ReturnPathOnlyWhileOpen`,
  `DependenciesPointBackwards`, `NoOpenTaskOnDeadAssignee`, `NoStaleActiveGoal`, `ReservationReleased`,
  `OneActiveRequest` (counting turn requests only: compression runs alongside and does not move the phase),
  `SelectionIsComplete`, `ResolvedWaitIsAnswered`, `NoEffectBeforeApproval`, `LiveIsPersisted`, `TailAppend`,
  `NoEntryIsEverLost`, `CoveragePointsForward`, `CoverageNeverLifted`, `NewestSummaryIsVisible`,
  `ApprovalDecisionIsFinal` (a decided approval is never rewritten; PENDING → expired is legal),
  `PendingApprovalOnlyForPreparedOperation` (RT-06: no pending approval survives a terminal operation),
  `NoEffectAfterDenial`, `ReceiptsAreStable` (a stored receipt for a command id is never rewritten),
  `ReplayedCommandIsInert` (a replay step must not move the command, event or context tables), `LogMonotone`
  (the event log only grows) and context-epoch consistency;
- **Coverage assertions**: the walk must really reach "a wait resolved / a settled goal / a settled task / a
  terminal operation / an epoch reset / a terminated instance / a LIVE artifact / a submitted compression / a
  decided approval / a command replay (same id returns the stored receipt, a different payload is refused)",
  otherwise the test fails (this prevents a vacuous pass);
- **Negative control** (`the_invariant_checker_detects_broken_states`): when state is broken on purpose
  (unknown status values, a rewritten terminal state, a stale goal pointer) the checker must report it,
  otherwise "everything passed" means nothing.

This correspondence has already caught two code issues (V-P1 and V-P2 below) and covers several
"must be refused" counterexample probes (delegating into a settled goal, settling a task as a non-assignee).

Boundary: this is **bounded enumeration plus sampling**, not a proof. It checks whether implementation states
satisfy the invariants; it does not check liveness and does not cover concurrent interleavings
(`Control::submit` is serialized on one connection; interleavings belong to the driver layer).

### Finding V-P2 (found by the code-level invariants, fixed)

The random walk reached "import a compression request as a turn": `import_response` checked only that the
request was `PENDING`, never its `kind`, so a compression request could be imported into the context as a turn
(append an assistant entry, open operations, close out as a turn) although a compression request may only be
submitted by `compress_context` as a summary (§7/A20). The driver never calls it that way, but the control
plane did not refuse it. Fix: `import_response` refuses requests whose `kind != 'turn'` and points at
`compress_context`; regression `import_response_refuses_a_compression_request`.

### Finding V-P1 (found by the code-level invariants, fixed)

Terminating an instance cancelled the in-flight request through `close_epoch_execution`, but the TERMINATED
branch of `set_lifecycle` did not reset the execution pointer the way `reset_instance`/`fail_request` do: the
instance stayed at `phase = MODEL_PENDING` with `active_request_id` pointing at a `CANCELLED` request, so
"phase is `MODEL_PENDING` implies a PENDING request exists" stopped holding for a terminated instance. Fix:
the termination branch applies the same normalization (phase → READY, pointer cleared); regression
`terminating_an_instance_normalizes_its_execution_pointer`.

## Bounded enumeration of the pure functions (`core/tests/kernel_properties.rs`)

The part of the kernel that never touches the database (wire view, output capping, paging, response
classification) is checked by bounded enumeration, also as part of `make check`:

```bash
cargo test --offline --manifest-path core/Cargo.toml --test kernel_properties
```

| Check | Property |
|---|---|
| `wire_view_is_a_paired_permutation` | the output of `prepare_request`: the system prompt first, the rest a **permutation** of the input entries (nothing lost, nothing duplicated), every answered call immediately followed by its answer, and assistants keeping their relative order. It enumerates all 258 entry combinations of length ≤ 3 plus two longer cases, and asserts the pairing really moved something at least 10 times (otherwise the property would be vacuous) |
| `tool_output_cap_keeps_head_and_tail_within_bounds` | content within the cap is not rewritten; beyond it the length stays bounded (≤ the cap plus 64 for the truncation marker), head and tail are kept and truncation is marked |
| `paging_reconstructs_the_original_without_gaps` | page-by-page retrieval through `page_output` rebuilds the original **seamlessly** (every length 0..12 × limit 1..5); coordinates agree (`next_offset` equals the consumed length, eof has no further offset); invalid arguments and out-of-range requests fail loudly instead of truncating silently |
| `response_classification_is_exhaustive` | `interpret_response`: a lone finish → completion candidate; a lone wait → a wait; mixed with other calls → dropped with a protocol note while the rest still become intents; an empty response → an ordinary reply |
| `args_hash_is_deterministic` | equal arguments always produce the same `args_hash` (receipts, deduplication and replays rely on it) |

Two boundaries, recorded honestly rather than as defects:

- capping can **lengthen** input that barely exceeds the cap (head + tail + marker, at most +64 characters);
  real shrinking happens far beyond the cap;
- `pair_tool_results` only moves an answer **up** to behind its call, never down: an answer before its call
  cannot occur in a real log (the runtime appends the call first), so that path is covered by the permutation
  property alone.

## Kani proofs: paging arithmetic (`make verify-kani`)

`verification/kani/` is a crate used only by Kani: it compiles **the repository's own
`core/src/kernel/types.rs`** through `#[path]` (adding only a `models::now` shim that none of the proven
functions reads), so it proves the **published code**. It needs a local Kani toolchain
(`cargo install --locked kani-verifier && cargo kani setup`) and, like the TLA+ targets, **never runs inside
`make check`**.

| Proof target | Coverage |
|---|---|
| `page_span_never_overflows_or_overruns` | for **every `usize`** (no assumptions beyond `offset <= total` and `limit >= 1`): a page is at most `limit` long, `offset + page` neither overflows nor runs past the end, a full page is taken unless the tail is reached, the tail consumes exactly the remainder, and the eof test is equivalent to "the remainder fits in limit" |
| `empty_page_moves_nothing` | for every `usize`: at `offset == total` the page is empty and the cursor does not move |
| `paging_covers_the_whole_output_exactly_once` | small lengths within the unwinding bound: page-by-page retrieval has no overlap, no gaps and a bounded page count |

The proven `page_span(total, offset, limit) = min(limit, total - offset)` is the **published function**:
`page_output` takes exactly that many characters per page (`take(page_span(...))`), so the arithmetic property
"a page neither overruns nor overflows" covers the shipped code rather than a copied stand-in. A measured
`make verify-kani` run takes about 7 seconds and all three harnesses pass.

**Honest boundaries** (measured, not guessed):

- `page_output`'s **argument parsing** (through serde_json) is not covered by Kani: once coordinates are
  symbolic, numeric comparison degrades into a symbolic `memcmp` (measured: not converged after 2200+
  expansions) and expanding `chars().count()` bloats similarly. Argument validity therefore stays covered by
  concrete-value enumeration (the out-of-range and invalid-argument cases in
  `core/tests/kernel_properties.rs`).
- `cap_tool_output`'s 24000-character threshold would need 24000 levels of unwinding, which Kani cannot do; it
  is covered by concrete tests at boundary lengths.
- Toolchain: Kani 0.68.0 with CBMC 6.11.0; Kani installs the nightly it pins (this machine uses
  `nightly-2026-08-21`).

## Boundaries (stated honestly)

- What is verified are **model** properties: TLC enumerates an abstract state machine, not the Rust
  implementation. Without a refinement proof (an optional later phase) this cannot be turned into "the Rust
  code is proven".
- Modelled: the control-plane state machine, artifacts and GC (A30), waits/wakeups/timers/supersede
  (A22/A23 and the RT-06 deduplication semantics), tasks/delegation/goal settlement (A02/A09), context
  compression (A20), the daemon protocol's command deduplication and snapshot watermark (A28), and the
  required-check rounds with repair/blocking (A16).
- Not modelled: the `expires_at` check of an approval (the resulting terminal state and "no pending approval
  after a terminal operation" are covered by the code-level invariants, but there is no separate TLA module);
  the execution details of `execute_check_ops` (dispatch/timeout/reconnect) are abstracted to "rounds and
  verdict". Cross-instance settlement of a shared goal budget (the worker attribution of A18) is modelled in
  `V2Task` through `budget_goal`'s resolution rules, including the "oldest open task only" ordering detail.
- Weak fairness: `V2Wait`'s liveness depends on weak fairness of the parked drain, i.e. the driver's poll loop
  continuing to try while `WAITING` (`engine/src/v2/driver.rs`); that is an implementation fact, not a proven
  conclusion.
- State-space frontier (`MC_task`): 1 task / 2 instances / 2 goals = 5.7M states in about 20 seconds; a second
  task diverges (measured: 43M states without convergence after four minutes) and needs symmetry or a stronger
  abstraction.
- The code-level correspondence (`core/tests/v2_invariants.rs`) is sampling plus bounded enumeration, not a
  proof: it gives "these executions satisfy the invariants" plus checker sensitivity (the negative control),
  never "all executions do".
- State-space frontier: the wide configuration is 275M states in 11 minutes; more instances or operations need
  symmetry, constraints or random simulation (`-simulate`) as a supplement.

## Conclusions and ledger

- [REPORT.md](REPORT.md): the conclusions of the formal verification (what can and cannot be claimed), the
  evidence list, the per-item A01–A36 ledger, the unproven list and the conditions that would overturn it.

## Phase status and optional upgrades

All four planned phases are complete; conclusions and the ledger are in [REPORT.md](REPORT.md):

1. ~~Extend spec coverage~~: seven protocol surfaces are modelled (control plane, artifacts, waits, tasks,
   compression, daemon, required checks), each mapped to code anchors and acceptance items.
2. ~~Code-level invariant tests~~: landed as `core/tests/v2_invariants.rs` (bounded enumeration plus
   coverage-driven random walks, coverage assertions and the checker-sensitivity negative control). proptest
   was not introduced, because enumeration plus fixed-seed walks already give reproducible equivalent evidence
   and stay lazy-first.
3. ~~Pure-function layer~~: `core/tests/kernel_properties.rs` (bounded enumeration) plus `verification/kani`
   (the Kani proof of the published `page_span`).
4. ~~Verification report~~: [REPORT.md](REPORT.md).

Still open as **upgrades** (not unfinished requirements, but optional depth):

- Lean 4: an interactive prover that needs the elan toolchain and hand-written scripts; it would turn the
  pure-function layer from "bounded enumeration plus bounded Kani proofs" into unbounded theorems, at the cost
  of a narrower surface than "one more enumerated protocol surface".
- A second task in `MC_task`: needs symmetry or a stronger abstraction to converge (today 1 task / 2
  instances / 2 goals = 5.7M states).
- The `expires_at` check of approvals and the execution details of `execute_check_ops`: currently covered only
  by the code-level invariants and sample tests.
- A refinement proof (model → implementation): needs every invariant mapped to an executable code assertion
  (done) plus a proof that each implementation step lies in the model's step set (not done, see REPORT.md §5).
