# Formal verification (control plane and beyond)

Goal: turn the **confirmed protocol properties** of the [design](../docs/DESIGN.md) into machine-checkable
specs instead of relying on sample tests alone.

Verification has three layers, all material sitting in this directory: (1) **protocol models** — TLA+/TLC
specs (`tla/V2*.tla` plus `MC*.cfg`) abstracting the protocol behaviour of `core/src/v2/control.rs` and
`engine/src/v2/driver.rs`; (2) the **executable spec-to-code correspondence** — `core/tests/v2_invariants.rs`
recomputes the invariants over real command sequences; (3) the **pure-function layer** — bounded enumeration in
`core/tests/kernel_properties.rs` plus Kani proofs in `kani/`. The TLA+ model is **not a refinement proof**:
results on the model do not automatically hold for the code, and code-side results come from bounded
exploration. The boundaries are in "Boundaries" below and in [REPORT.md](REPORT.md). The two positive targets (`verify-model*`) and `verify-kani` **require their success marker** — a TLC run that prints
"No error has been found", a Kani summary with zero failures — because their output also contains the failure
words, and a recipe whose status was a `grep` for those could pass while a property was violated (D-122). A run
that prints a `Warning:` is a failure too (D-206): TLC reports an inconsistent model — an `UNCHANGED` list that
contradicts an assignment, so the action is inert — as a warning and still says `No error has been found`, which is
how the first version of `V2Jobs` had a counterfactual that silently *verified* instead of refuting. One ordering
note (D-202): §0 pins the commit the gates were last run at, and the rule that checks it reads the *committed*
material — so a change to `tla/` or `kani/` lands one commit before the pin that documents its re-run, which is the
shape D-206 was committed in.

## Running

```bash
make verify-model           # small control-plane configuration (seconds)
make verify-model-all       # small configurations for all fifteen modules (control plane, artifacts, waits,
                            # tasks, compression, daemon, required checks, authority, the user's surface,
                            # session-store identity, retention, the job handshake, the inbox, the write-failure
                            # latch, the coordinator lock)
make verify-model-counterexamples   # the negative controls (authority surface D-61, inbound boundary D-63,
                            # the retry boundary D-64/D-65, the runtime's own closing word D-71, the
                            # landing-attribution rule D-72, the store-identity guard D-87, the four retention
                            # guards D-192, the four job-handshake rules D-206, the five inbox rules D-207, the four write-failure
                            # rules D-208 and the three coordinator-lock rules D-210):
                            # each must be *refuted*, or the property it targets proves nothing
make verify-model-wide      # wide control-plane configuration (2 instances / 2 operations; tens to hundreds of
                            # millions of states, slow — the 2-instance run is what catches per-instance
                            # fairness regressions. Beyond a bounded attempt: D-210)
make verify-model-sim       # any configuration by random simulation (~2-4 minutes, 20k behaviors of depth
                            # 100): the *invariants* only — simulation checks no temporal property — and a
                            # violation found here is real while absence proves nothing (D-211, D-215).
                            # `SIM_CONFIG=…` picks one, including the refuted controls
make verify-kani            # paging arithmetic (needs the Kani toolchain, see below)
cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants   # spec-to-code correspondence
```

The first run downloads the pinned `tla2tools.jar` (TLC v1.7.1, SHA-256 in the Makefile) into
`$TLA_TOOLS_DIR` (default `~/.local/share/teamagents-verify`) and verifies it; the toolchain never enters the
repository and never enters `make check`. Java is required (this machine uses OpenJDK 27).

## Specs and configurations

| File | Contents |
|---|---|
| `tla/V2Control.tla` | control-plane abstraction: instance phase machine, requests/attempts, decisions and operations, approvals, the dispatch linearization point, cancel/timeout, epoch resets, goal budget reservations and settlement, crash/recovery, and the inbound boundary (an input that arrives during a turn waits for it, D-63) |
| `tla/MC.cfg` | small configuration (1 instance / 1 operation / 2 request slots / 1 attempt slot / 1 epoch reset / 1 unknown usage) |
| `tla/MC_control_midturninput.cfg` | negative control for D-63: the driver applies input *inside* the running turn (the pre-D-63 behaviour). `make verify-model-counterexamples` requires TLC to refute `InputLandsAtTheBoundary` here |
| `tla/MC_control_two.cfg` | two-instance control-plane configuration (same domains as `MC.cfg`): the small check that tells a *per-instance* fairness or liveness assumption apart from one that lets one instance be starved by the other's progress |
| `tla/MC_control_two_disjunction.cfg`, `tla/MC_control_deadline.cfg`, `tla/MC_control_reask.cfg`, `tla/MC_control_runtimeTail.cfg`, `tla/MC_control_landing.cfg` | negative controls (D-63/D-64/D-65/D-71/D-72): fairness as one disjunction over instances, a runtime that ignores the goal's deadline, a runtime that re-opens a turn while the last word is the model's own (the pre-D-65 code), one that treats the runtime's own closing note as unaddressed work (the shape the pre-D-71 idle rule implied), and one that stores an input *inside* the running turn so its settlement belongs to a turn that never saw it (the pre-D-63 shape, and the premise the D-72 attribution rule rests on). All five must be refuted by `make verify-model-counterexamples` |
| `tla/MC_wide.cfg` | wide control-plane configuration (2 instances / 2 operations with one requiring approval / 3 request slots / 2 attempt slots) |
| `tla/V2Artifact.tla` + `tla/MC_artifact.cfg` | artifacts and GC: write bytes → STAGING row → reference and LIVE in one transaction → GC claim → the caller deletes the bytes → the row is collected (`artifact_collect`), with abandon for a STAGING orphan; the runtime's half runs at a driver's boot (D-191). `tla/MC_artifact_gc_ignores_references.cfg` is its counterfactual control (D-220): the collector never reads the reference table, so an artifact messages still point at is claimed, and `GcClaimsOnlyUnreferencedLive` is refuted |
| `tla/V2Retention.tla` + `tla/MC_retention.cfg` | retention as DESIGN states it, *before* an implementation (D-75 recorded the keys as accepted-but-unapplied, D-192 models the rule): a fact may leave the database only when retention is switched on (`history_days` = 0 keeps the full history), it is at least that old, no live reference protects it and it is not evaluation evidence — with the interleavings that make the rule worth checking (a reference attaching, detaching, evidence being marked while days pass). Its claims are named: `EvictionOnlyUnderTheGuards` (the guards evaluated at the step that evicts), `NoReferenceToEvictedFact` (no live reference outlives the fact it points at), `EvidenceIsNeverEvicted` (evidence is never evicted, however old) and `OnlyOldFactsAreEvicted` (the state half: a fact that is gone was old and retention was on). The negative controls `tla/MC_retention_evicts_live.cfg`, `tla/MC_retention_evicts_evidence.cfg`, `tla/MC_retention_evicts_young.cfg` and `tla/MC_retention_runs_disabled.cfg` each forget one guard and must be refuted by `make verify-model-counterexamples`; it is safety only, because the design does not promise a cleanup ever runs |
| `tla/V2Jobs.tla` + `tla/MC_jobs.cfg` | the job handshake and the recovery verdict (§6.2/§6.3, A10/A11; D-91/D-112/D-153): the journal's phases (READY, START_ACCEPTED, RUNNING, CANCEL_REQUESTED, terminal), the acceptance persisted *before* the spawn (`NoEffectBeforeAccept`, `EffectImpliesAcceptedStart`), a duplicate GO as a no-op (`AtMostOneExecutor`, reading the journal's own `starts` counter), CANCEL before the start as final (`LateGoIsRejected`), and the recovery read — one atomic snapshot of the journal and the effect — that says "did not run" only for a READY journal and records `unknown` for the START_ACCEPTED/RUNNING/CANCEL_REQUESTED band (`UnverifiableStartIsNeverGuessedNotRun`, `NotRunMeansNoEffect`, whose counterexample is the accept-after-spawn shape); `SettledRunnerLeaves` (under weak fairness of the shutdown step) is D-153's rule that a settled job's runner goes away rather than idling forever. The four negative controls `tla/MC_jobs_guess_notrun.cfg`, `tla/MC_jobs_double_go.cfg`, `tla/MC_jobs_late_go.cfg` and `tla/MC_jobs_spawn_first.cfg` each forget one rule — a missing pid read as "did not run", a duplicate GO starting a second command, a late GO after CANCELLED_BEFORE_START starting it anyway, and a command spawned before its acceptance was persisted (which is what makes a run invisible to recovery) — and must each be refuted by `make verify-model-counterexamples`. The model abstracts pid, start_ticks and boot_id into one "the recorded identity verifies" variable and assumes authenticated control requests; the deadline, the TERM→KILL escalation and the identity refusal are the runner's own tests and the live probes (`review/dogfood/job_identity.py`, `crash.py`, `unknown_outcome.py`, `cancel.py`) |
| `tla/V2Inbox.tla` + `tla/MC_inbox.cfg` | the inbox and the application of an envelope (§5.3, A06, A24; D-63/D-72): an envelope is persisted as accepted, the recipient applies the lowest pending envelope of the current epoch **once** — the append carries the envelope id as its dedup key, so a replay appends nothing (`AtMostOncePerEnvelope`) — in sequence order (`LogIsIncreasing`), a stale-epoch envelope seals instead of leaking into the new epoch (`NoStaleApplication`), nothing accepted is ever silently dropped (`NoSilentLoss`), no send is accepted into a full inbox (`BoundedInbox`, the bound `queue_envelope` checks; a lost marker may legitimately leave more rows pending than the cap), and only the user or the instance itself drains (`DrainIsOwned`). Its five negative controls `tla/MC_inbox_no_dedup.cfg`, `tla/MC_inbox_drop_when_full.cfg`, `tla/MC_inbox_unbounded.cfg`, `tla/MC_inbox_stale_applied.cfg` and `tla/MC_inbox_foreign_drain.cfg` each forget one rule and must be refuted. The model is safety only (no fairness: whether a drain runs is the client's business) |
| `tla/V2DiskFull.tla` + `tla/MC_diskfull.cfg` | the write-failure latch (§4.4, A31): a step whose submit fails with `StorageFull` latches, the driver stops running steps — the failed step is never retried and new side effects stay stopped (`NoStepRunsWhileLatched`) — success is never faked (`NoFakedSuccess`), the park is retried at poll pace and its reason names the in-flight persistence loss (`LossIsReported`), the latch clears only because that park landed — which needs a write to land (`LatchClearsOnlyWhenWritable`) — and the user resumes once space is freed (`ParkEventuallyLands`, under strong fairness of the park: the retry is live *while writability recurs*, because a single writable moment proves nothing). Its four negative controls `tla/MC_diskfull_keep_driving.cfg`, `tla/MC_diskfull_fake_success.cfg`, `tla/MC_diskfull_park_silently.cfg` and `tla/MC_diskfull_clear_anyway.cfg` each forget one rule and must be refuted |
| `tla/V2Coordinator.tla` + `tla/MC_coordinator.cfg` | the coordinator lock and its fork window (§6.1, A33): one coordinator per state root (`AtMostOneCoordinator` — the kernel grants the lock to one holder), a restart that lands while only a *tool child's* inherited copy holds it waits for `exec` instead of reporting a coordinator that is not really there (`TheWindowIsNotMistakenForAHolder`), and with no coordinator alive the only thing that may still hold the lock is a child in that window (`NoHeldLockWithoutAHolder` — nothing inherits it past `exec`), with `RecoveryIsLive` for the poll-pace retry. Its three negative controls `tla/MC_coordinator_report.cfg`, `tla/MC_coordinator_inherit.cfg` and `tla/MC_coordinator_shared.cfg` each forget one rule and must be refuted |
| `tla/V2Wait.tla` + `tla/MC_wait.cfg` | waits/wakeups/timers/supersede: evaluate at registration → parked drain scan → answer in the same transaction when satisfied → cancel/supersede/re-arm. `tla/MC_wait_closes_without_answering.cfg` is its counterfactual control (D-220), and it restores the refutation finding V-W1 is recorded against: both non-drain exits close the wait and answer nothing, which `ResolvedWaitIsAnswered` refutes — the pre-fix shape had lived in `MC_wait_contract.cfg` until that was renamed into the positive `MC_wait.cfg`. Two more claims it carries: `SatisfiedHoldsConditions` (a SATISFIED wait really had its conditions hold — no spurious wake; the answer of a CANCELLED wait is a cancellation notice) and `AnswerImpliesResolved` (an answer only ever joins the context together with a resolved wait) |
| `tla/V2Task.tla` + `tla/MC_task.cfg`, `tla/MC_task_two.cfg` | task lifecycle and goal settlement: delegation (dependencies must exist first, the goal must be ACTIVE) → start → settle/cancel → system parking → termination cascade; goal creation, request admission, open operations, settlement and detach. `tla/MC_task_two.cfg` adds the second task slot at one instance and one goal, which is what makes `DependenciesPointBackwards` and `NoSelfDependency` non-vacuous (with one task a prerequisite can only be empty). Its four counterfactual constants (D-218) each name a plausible mistake and must be refuted: `tla/MC_task_delegates_to_settled.cfg` (delegation onto a settled goal — finding V-G1 — refutes `RegisteredWorkNeedsAnActiveGoal`), `tla/MC_task_bills_settled.cfg` (a request billed to a settled goal refutes `RequestsResolveToActiveGoals`), `tla/MC_task_unordered_dependency.cfg` (a prerequisite that is the task itself or a later one refutes `DependenciesPointBackwards`, and with it `NoSelfDependency`) and `tla/MC_task_terminate_leaves_tasks.cfg` (a termination that leaves the instance's open tasks open refutes `NoOpenTaskOnDeadAssignee`) |
| `tla/V2Compress.tla` + `tla/MC_compress.cfg` | context compression (A20): open/submit/fail/cancelled by a closed epoch; summaries append at the tail, coverage only grows, originals are never deleted and a request closes only once (`RequestClosesOnce`, which D-212 found unlisted — and which D-219 found *entailed by `TypeOK`*: it was a disjunction over `RequestState` and could not fail, so the transition property it names was unmodelled until it became a monitor). Three counterfactual constants and their controls: `tla/MC_compress_deletes_originals.cfg` (a summary deletes the entries it covers → `NoEntryIsEverLost`), `tla/MC_compress_lifts_coverage.cfg` (a commit lifts coverage → `CoverageNeverLifted`) and `tla/MC_compress_rewrites_closed.cfg` (a late failure rewrites a closed request → `RequestClosesOnce`) |
| `tla/V2Daemon.tla` + `tla/MC_daemon.cfg` | session daemon protocol (A28): deduplication and replay of stable command ids, the atomic snapshot+watermark pair of `checkpoint`, gap-free `events(since)`, a slow client never blocking the writer. Three counterfactual constants and their controls (D-219): `tla/MC_daemon_rewrites_receipt.cfg` (a same-payload replay re-applies and moves the receipt → `ReceiptsAreStable`), `tla/MC_daemon_rolls_back_log.cfg` (a compaction drops the oldest version → `LogMonotone`) and `tla/MC_daemon_reclaims_events.cfg` (events are reclaimed → `NoResyncInThisVersion`) |
| `tla/V2Checks.tla` + `tla/MC_checks.cfg` | required checks (A16/§8): only self-reported successes are verified, failures enter a bounded repair round, an exhausted budget or an unusable verification path (stale observation, refused dispatch) parks the goal BLOCKED, and a candidate is never upgraded. `tla/MC_checks_rewinds_rounds.cfg` is its counterfactual control (D-219): opening a round resets the counter instead of advancing it, which is what `RoundsAreMonotone` forbids |
| `tla/V2Grants.tla` + `tla/MC_grants.cfg` | authority (§5.1/§6.1, A03/A04; D-58/D-59/D-60): the session's bootstrap grants, narrowing by an instance (manage covers message/delegate), the spawn-derived delegate grant, revocation with the parent tree cascade and the revision bump, the dispatch re-check, and the rule that the model-visible tool surface only offers what the instance's grants back. `tla/MC_grants_stale_offered_surface.cfg` is its counterfactual control (D-220): the revoke path computes the surface from the grant table as it was before the revocation, so a model is still shown a tool whose grant went away, which `OfferedToolsAreAuthorized` refutes. Its type claim is the composite `TypeOK`, listed beside its four components `TypeOKGrants`, `TypeOKRevision`, `TypeOKOffered` and `TypeOKOps` (D-212's convention: a claim that composes others is listed itself and its parts beside it) |
| `tla/V2Store.tla` + `tla/MC_store.cfg` | session-store identity (A34; D-87): opening a state root either owns the file or refuses it — a foreign program's tables are never stamped as ours, a refusal writes nothing, and a database interrupted between its schema batch and its stamp is completed rather than stranded. `tla/MC_store_adopt.cfg` is its negative control |
| `tla/V2Authority.tla` + `tla/MC_authority.cfg` | the user's authority surface (D-61): the view a client reads (and the id a revoke must name), the pair table the surface refuses against, a grant the user writes (optionally derived from one it holds), revocation by a nameable id with the subtree cascade, the surface as a *cached* per-request variable, and the dispatch re-check with a surface that may lag. Its three negative-control configurations (`MC_authority_badview.cfg`, `MC_authority_trustsurface.cfg`, `MC_authority_stalesurface.cfg`) are run by `make verify-model-counterexamples` and must each be refuted |

Two control-plane configurations exist: `MC.cfg` (one instance, the default in `make verify-model-all`) and
`MC_control_two.cfg` (two instances, the same domains — the small check that makes a *per-instance* fairness or
liveness assumption testable, which the slower wide configuration cannot). The environment (tool results,
approval timing, crash points, and whether a turn is in flight when input arrives) is **non-deterministic** in
the model; that is exactly what is enumerated.

## Verified properties and their code anchors

| Property (spec) | Meaning | Code anchor | Acceptance |
|---|---|---|---|
| `TypeOK` | phase/lifecycle/operation status/effect counters hold legal values | the `models.rs` enums, `OpStatuses` | §4.1 |
| `NoEffectBeforeApproval` | an operation needing approval has no effect before it is approved | the approval gate in `dispatch_operation`; `approve`/`deny` (reachable headlessly through `teamagents approvals`, D-67) | A25/A12 |
| `RecordBeforeEffect` | a persisted dispatch record exists before any effect | `dispatch_operation` writes `DISPATCH_COMMITTED` first | A08/A11 |
| `EffectAtMostOnce` | an operation has at most one external effect (recovery never replays) | the recovery path only sets `OUTCOME_UNKNOWN` | A08/A10/A13 |
| `ReservationsAdmitted` | live reservations never exceed the ceiling (a consequence of the admission gate) | `known+reserved+est ≤ max` in `reserve_budget` | A18/§8 |
| `AdmissionGate` (temporal) | every entry into `MODEL_PENDING` passed the admission gate | as above | A18/§8 |
| `ReservationReleased` | closing a request (complete/fail/cancel) always releases its reservation | the `release_reservation` call sites | §8 |
| `OneActiveRequest` | an instance has a single active request at a time | the phase/revision guard in `begin_request` | §3/§6.1 |
| `SelectionIsComplete` | only an atomically selected complete attempt exists | the `selected_attempt_id IS NULL` update in `record_attempt` | A19 |
| `NoTurnWithoutWork` | no new turn opens while the last word is already committed — the model's own text **or** the runtime's own closing note | the closing entry plus the idle test in `step_ready` (D-65: it counts only *unaddressed* work, so an open task no longer re-asks the model; D-71: a settlement is a committed tail, `EntryKind::Runtime`) | §5.4/§3/§8 |
| `NoRequestAfterDeadline` (temporal) | a goal past its deadline begins no new request | the `goal_deadline_passed` gate in `begin_request`/`begin_compression` plus the driver's park (D-64/A35) | A35 |
| `InputLandsAtTheBoundary` | user input only ever enters the context at a READY boundary — never inside a turn whose request is already fixed | `submit_input` queueing while `MODEL_PENDING`/`TOOLS_PENDING`/`COMPLETION_PENDING`, and the boundary drain in `step_ready` (D-63) | §5.4/A21 |
| `SettlementFollowsATurnAfterTheLanding` | an outcome belongs to the turn that produced it: the goal can only settle after a request begun since the instance's last input landing | the boundary drain before `BeginRequest`, `complete_goal` reading a decision's candidate, and the client's positional attribution in `exec` (D-72) | §5.3/§5.4/§8 |
| `QueuedInputEntersTheContext` (temporal) | an input that waited for the boundary enters the context; it is never dropped while the instance keeps running (a park keeps it, a reset seals it with its epoch, termination ends it) | the `envelopes` state machine (`ACCEPTED` → `APPLIED`, sealed at a reset) plus the drain | A06/A21 |
| `StaleExecutorRejected` | an executing instance holds the current revision | `revision == expected` in `begin_request` | §6.1 |
| `NoEffectOnTerminated` | a terminated instance produces no effect | TERMINATED in `set_lifecycle` plus the dispatch guard | §6.4 |
| `PreparedIsNotTerminal` | a `PREPARED` operation has no effect yet | the operation state machine | §6.1 |
| `CancelledBeforeStartHasNoEffect` | "cancelled before start" means no effect (a dispatch record may exist) | `cancel_operation` on `DISPATCH_COMMITTED` | A13 |
| `TerminalOpStable` (temporal) | a terminal operation is never rewritten | the "already terminal" refusal in `complete_operation` | A13 |
| `TerminalGoalStatusStable` (temporal) | a terminal goal is never rewritten | the `already_closed` branch of `complete_goal`/`block_goal` | §8 |
| `NoReceiptAcrossEpochs` (temporal) | a receipt never lands across epochs | `reset_instance` closes the old epoch and cancels in-flight operations | A24 |

### Authority (A02/A03/A04, §5.1/§6.1, D-58/D-59/D-60)

| Property (spec) | Meaning | Code anchor | Acceptance |
|---|---|---|---|
| `AuthorizedEffectsOnly` (temporal) | every effect is produced by a dispatch that held the authority at that step: a live covering grant **and** a still-current stamped revision | the guard of `dispatch_operation` (`grant_revision == permission_revision` plus `capability_gap`) | A03/A04 |
| `EffectAtMostOnce` | an operation has at most one effect | `complete_operation`'s terminal-state guard and the recovery path | A08/A10 |
| `OnceStaleNeverExecutes` | an operation refused at an outdated revision can never take effect afterwards | the revision only grows; a refusal is terminal for that operation | A04 |
| `ChildGrantsAreCoveredByTheirParent` | a derived grant never exceeds its parent (its action and scope are covered) | the parent check in `issue_grant` | A03 |
| `AuthorityTracesToTheUser` | authority is never invented: every live grant traces back to one the user issued | `issue_grant`'s identity rules (`Identity::System` refuses; an instance must hold a covering grant) | A02/A03 |
| `RevokedStaysRevoked` / `CascadeTakesTheSubtree` | revocation is final and takes the whole subtree | `revoke_grant` + `revoke_grant_tree`, `revoked_at IS NULL` in every check | A03 |
| `OfferedToolsAreAuthorized` | the model-visible surface never offers a tool the instance cannot dispatch | `driver::team_kernel` derives the collaboration tools and the shell tool from the instance's grants | §5.2/D-60 |
| `BootstrappedAuthority` | the session boots with exactly the documented authority (leader: shell@workspace + manage/delegate/message@session; a spawned child holds none of it) | `driver::bootstrap` (D-58) and `create_instance`'s workspace grant | A01/A02 |

### The user's authority surface (`V2Authority`, A02/A03, D-61)

`V2Grants` models where authority comes from; this module models the surface a user drives it with — the
questions that only exist once the user can. Each claim is paired with the control that must refute it
(`make verify-model-counterexamples`), because a property that cannot fail proves nothing.

| Property (spec) | Meaning | Code anchor | Control that refutes it | Acceptance |
|---|---|---|---|---|
| `NoDeadGrantPair` | the pairs the surface accepts are exactly the pairs some check asks about: a grant nothing consults is refused with a reason instead of written | `core/src/v2/capability.rs` (`ACTIONS`, `asks_about`, `authorizes_something`) and `authority.rs`'s guards | — (a constant table equality) | A03 |
| `EveryLiveGrantBecomesRevocable` (temporal) | every live grant eventually appears in the view, so it can be named and revoked | `daemon.rs` `read_method("grants")` carrying `id`; `authority.rs` revoking by id or unambiguous prefix | `MC_authority_badview.cfg`: the view without the `id` field — the daemon before D-61 | A03 |
| `CascadeOnlyTakesTheSubtree` (temporal) | revoking one grant takes exactly its subtree: the worker's grant dies with its parent, and the leader's authority does not | `revoke_grant_tree` and the `--parent` check in `issue_grant` | — | A03 |
| `AuthorizedEffectsOnly` (temporal) | every effect comes from a dispatch that re-read the live grants at that step, even though the cached model-visible surface may lag | the `capability_gap`/grant-revision guard in `dispatch_operation` (§6.1/A04) | `MC_authority_trustsurface.cfg`: dispatch trusts the cached surface | A04 |
| `StaleSurfaceCatchesUp` (temporal) | a surface that lags the entitlement catches up at the instance's next request — the grant really becomes visible | `driver::team_kernel` recomputing the tool list per request; `v2_supervisor::a_users_grant_reaches_the_workers_surface_at_the_next_request` | `MC_authority_stalesurface.cfg`: the surface computed once and never recomputed | §5.2/D-61 |
| `SurfaceChangesOnlyToTheCurrentEntitlement` (temporal) | a surface change is always the entitlement of that moment: recomputed, never invented, never carried over | as above | — | §5.2 |
| `ListedIdsAreUsed` | the view never invents an id | the `grants` SELECT | — | A03 |
| `AuthorityTracesToTheUser`, `RevokedStaysRevoked`, `ChildGrantsAreCoveredByTheirParent`, `CascadeTakesTheSubtree`, `EffectAtMostOnce`, `OnceStaleNeverExecutes`, `BootstrappedAuthority` | as in `V2Grants`, in the presence of the surface's own grants | see the authority table above | — | A02/A03 |

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
| `AnswerImpliesConditions` | no spurious wakeups: an answer is appended only when the conditions really hold | `evaluate_wait` tests `satisfied` before appending; a task condition accepts `SUCCEEDED`/`FAILED`/`CANCELLED`, so cancelling a task releases its delegator (`v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance`, D-65/D-68) |
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
| `ReturnPathOnlyWhileOpen` | the narrow return path exists only while the task is unsettled; `SUCCEEDED/FAILED` and cancellation revoke it, `BLOCKED` keeps it | the `revoke_grant_tree` call sites; `terminal = SUCCEEDED`/`FAILED` |
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

Spec counterexamples (before the fix; `MC_task.cfg` guards both and TLC is green — and since D-218 the first is
*refutable again*, so a regression that reopens it fails `make verify-model-counterexamples` rather than passing
unnoticed):

```text
Error: Invariant RegisteredWorkNeedsAnActiveGoal is violated.     # a task was delegated before its goal existed
Error: Invariant ClosedGoalTakesNoNewOperation is violated.       # a settled goal still opened an operation
```

The two lived in the configuration the fix renamed into the positive `MC_task.cfg`, which left no refutation
behind; D-218 brought the first back as the counterfactual `DelegateToSettledGoal` and the control
`tla/MC_task_delegates_to_settled.cfg` (it reports the same invariant), and the second property went with the
spec that named it.

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

### Session-store identity (A34, D-87)

| Property (spec) | Meaning | Code anchor |
|---|---|---|
| `NoForeignAdoption` | a file that holds another program's tables is never stamped as a session store, however often it is opened (monitor `adoptedForeign`) | `store::open`'s unstamped branch consults `foreign_tables` before writing anything |
| `NoForeignStamp` | the same claim over the file itself: a v2 stamp never sits on a file with foreign tables | as above |
| `RefusalsWriteNothing` | a refusal is silent — it never writes to the file it refused | the stamp is *read* through `sqlite_master`, and the writing pragmas (WAL, `synchronous`) are applied only after the format check accepts the file |
| `InterruptedWroteOursOnly` | the crash path leaves our own tables and no foreign ones | the schema batch and the stamp insert are separate writes; the next open completes what it finds |
| `AcceptedMeansStamped` | an accepted or initialized open ends on a v2 file | the format check precedes every acceptance |
| `HalfInitializedIsCompleted` (leads-to) | a database interrupted between its schema and its stamp is completed rather than stranded, given weak fairness of the initializer | `create = true` on an unstamped file whose tables are all the schema's own |
| `NoForeignStampAlways`, `EveryRefusalIsSilent` (temporal) | the two safety claims in `[][…]` form | as above |

Non-vacuity evidence: `tla/MC_store_adopt.cfg` is the D-87 defect (`create = true` initializing whenever there
is no stamp) and `make verify-model-counterexamples` requires TLC to report
`Invariant NoForeignAdoption is violated` — it does. The byte-level side of "refusals write nothing" is checked
where a model cannot see it: `open_never_adopts_an_unstamped_file_that_holds_foreign_tables` compares the file
with what it held before, and `review/dogfood/boundary.py` hashes it across a real `exec`.

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
  compression (A20), the daemon protocol's command deduplication and snapshot watermark (A28), the
  required-check rounds with repair/blocking (A16), the job handshake with its recovery verdict (A10/A11), and
  the inbox: the exactly-once application of an envelope, the sequence order, the bound, the stale-epoch seal and
  the drain's identity check (§5.3, A06/A24), the write-failure latch with the park it retries (§4.4, A31), and
  the coordinator lock with its fork window (§6.1, A33).
- The inbox model (`V2Inbox`) models one recipient's inbox over three envelopes and two epochs, identifies an
  envelope id with its sequence number (arrivals are ordered), and models one application step per envelope, so
  "at a safe boundary" is a step here and the driver's boundary machinery stays with `V2Control`. Senders and the
  `kind` vocabulary are not modelled (every kind obeys the same bound and no-loss rule), and DESIGN's permission
  for status notes to coalesce has no implementation to model. `CrashLosesMarker` is the window the committed
  code closes with one transaction; the model keeps it because the dedup, not the caller's transaction
  discipline, should be what makes a replay safe.
- The write-failure model (`V2DiskFull`) models one instance, one in-flight effect and no cost side (an artifact
  write failing latches through the same signature, so "a write" is one thing here), no storage-worker queueing,
  and no operation-ledger states after a lost outcome (that story is `V2Control`'s). Its liveness property says
  what the design actually promises: the poll-pace retry is live *while writability recurs*, not that a single
  writable moment is enough — the disk can refill before the next poll.
- The coordinator-lock model (`V2Coordinator`) abstracts the descriptor table to one counter (how many processes
  reference the description that holds the lock) and one tool child — a shell-job runner, an MCP server or a hook,
  which all fork the same way — and it does not model the poll interval, the lock file's path, the OS's own lock
  semantics beyond "exactly one holder", or what the child does after exec.
- The job model (`V2Jobs`) abstracts the three identity fields (pid, `start_ticks`, `boot_id`) into one "the
  recorded identity verifies" variable, assumes the control requests are authenticated (the abstract socket name
  is derived from the job token, which is a transport property), and does not model the deadline, the TERM→KILL
  escalation, the identity refusal against the job file, or the journal's bytes: those are the runner's own tests
  plus `review/dogfood/job_identity.py`, `crash.py`, `unknown_outcome.py` and `cancel.py` against a real runner.
- The authority model abstracts the scope vocabulary to the three kinds the code uses ("session" covers everything
  below it, "workspace", and one scope per instance), models the two operations that matter (the leader's
  delegation and a spawned child's shell call) rather than every tool intent, and assumes the instance a spawn
  creates exists (the instance-creation transaction is modelled in `V2Control`/`V2Task`).
- Not modelled: the `expires_at` check of an approval (the resulting terminal state and "no pending approval
  after a terminal operation" are covered by the code-level invariants, but there is no separate TLA module);
  the execution details of `execute_check_ops` (dispatch/timeout/reconnect) are abstracted to "rounds and
  verdict". Cross-instance settlement of a shared goal budget (the worker attribution of A18) is modelled in
  `V2Task` through `budget_goal`'s resolution rules, including the "oldest open task only" ordering detail.
- Weak fairness: `V2Wait`'s liveness depends on weak fairness of the parked drain, i.e. the driver's poll loop
  continuing to try while `WAITING` (`engine/src/v2/driver.rs`); that is an implementation fact, not a proven
  conclusion.
- State-space frontier (`MC_grants`): the four bootstrap grants plus one free slot, two instances and two
  operations = 1.29M states (178k distinct) in about one minute, with TLC's estimated chance that a fingerprint
  collision hid a state at 1.2e-9. It is the slowest of the small configurations; the others are seconds.
- State-space frontier (`MC_task`): 1 task / 2 instances / 2 goals = 5.7M states in about 20 seconds, and
  `MC_task_two.cfg` adds the second task with one instance and one goal — 612,802 states / 56,074 distinct in 8 s
  on 2026-09-27 — which is what makes the two dependency invariants non-vacuous, since a single task can only
  declare the empty prerequisite. The **whole product** (2 tasks / 2 instances / 2 goals, D-218) stays beyond a
  bounded attempt: measured 2026-09-27 at 33.6M states generated / 7.9M distinct after five minutes with the
  queue still growing, and the symmetry the report named is *not* enough — a sound block-preserving group over the
  three constant sets (declared as model values, which TLC requires; it was added to a copy of this module, since
  a configuration cannot carry the definition) cut the distinct count by only about 1.6× (31.5M / 5.0M after five
  minutes, queue still growing), because most states are not in general position under the group; two tasks with
  two instances (one goal) and with two goals (one instance) each also ran past five minutes. That configuration
  needs a stronger abstraction, not symmetry. The simulation supplement searches it
  instead — `make verify-model-sim SIM_CONFIG=MC_task.cfg`: 20,000 behaviors of depth 100, **7,663,011 states in
  2 m 13 s** on 2026-09-27 with no invariant violated — which is a search and not a proof, and it checks no
  temporal property.
- The code-level correspondence (`core/tests/v2_invariants.rs`) is sampling plus bounded enumeration, not a
  proof: it gives "these executions satisfy the invariants" plus checker sensitivity (the negative control),
  never "all executions do".
- State-space frontier: the wide configuration is 275M states in 11 minutes in the historical run, and a
  one-hour bounded attempt on 2026-09-27 did not reach a verdict (2.9 GB of state store written): the fields and
  properties added since then put it beyond a bounded attempt. The supplement named here is implemented as
  `make verify-model-sim` (its default, `SIM_CONFIG=MC_wide.cfg`): 20,000 random behaviors of depth 100,
  2,022,792 states checked in ~4 minutes on 2026-09-27 with no invariant violated — which is a *search*, not a proof, and it checks no temporal property
  (the small configurations do that exhaustively). More instances or operations still need symmetry or constraints
  — and symmetry alone was measured insufficient for the task product (D-218).

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
- A second task in `MC_task`: done where it converges (D-218 added `MC_task_two.cfg`, one instance and one goal,
  so the dependency invariants are no longer vacuous); the full 2 tasks / 2 instances / 2 goals product needs a
  stronger abstraction, since symmetry was measured at only ~1.6× (D-218) and the simulation supplement searches
  that shape meanwhile.
- The `expires_at` check of approvals and the execution details of `execute_check_ops`: currently covered only
  by the code-level invariants and sample tests.
- A refinement proof (model → implementation): needs every invariant mapped to an executable code assertion
  (done) plus a proof that each implementation step lies in the model's step set (not done, see REPORT.md §5).
