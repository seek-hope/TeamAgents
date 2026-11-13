# Current formal verification guide

This guide describes the current TLA+/Kani material and how to run it. Results and abstraction limits are in
[REPORT.md](REPORT.md); requirement/evidence mapping is in [REQUIREMENTS.md](REQUIREMENTS.md).

## Running

```bash
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
```

The first run may download the pinned **TLC v1.7.1** jar; its SHA-256 is pinned in `Makefile`. Keep generated states and manual probes under `tools/tmp/`.
Do not treat simulation, a timeout, a witness or a green exit without semantic checks as proof.

Current inventory: **393 configurations across 48 modules**: **65 positive configurations in 62 traversals**,
**327 controls**, and one separate wide configuration. There are **79 witnesses** and **9 Kani harnesses**.
Witnesses demonstrate reachability only. The wide configuration is not claimed complete, and Policy B canonical
allocation is not adopted.

## Session exploration tree

`V2SearchTree` models `read_search_tree`, `apply_search_tree` and compaction's tree patch. It checks
User/Leader authority, current-revision admission, stable replay, root/parent/acyclicity/focus, merge reference
union and reparenting, revision history, complete source watermarks, ordered pair atomicity and fail-soft
compaction. The second operation in a pair sees the first operation's result.

The positive families are `MC_search_tree.cfg`, `MC_search_tree_add.cfg`, `MC_search_tree_update.cfg`,
`MC_search_tree_move.cfg`, `MC_search_tree_merge.cfg` and `MC_search_tree_pair.cfg`. Update/move/merge/focus
start from a valid populated tree to avoid vacuity. Pair controls `MC_search_tree_pair_atomic.cfg` and
`MC_search_tree_pair_replay.cfg` reject partial application and replay growth. In total, 23 controls cover every
non-`TypeOK` tree claim and 11 witnesses cover normal operations and both compact outcomes.

Arbitrary strings, provider/JSON parsing, SQL execution and schema migration have executable Rust coverage, not
arbitrary-input TLA proofs. Live-model navigation quality is not established.

## Control-plane surface coverage

`core/src/v2/control.rs`'s `dispatch` is the set of methods `Control::submit` accepts — the protocol surface the
runtime and every client reach. This table names, for each method, the spec under which its protocol rule is
checked. `tools/verification_catalogue.py` compares the two directions: a method the code adds without a row
fails the audit, and a row whose method the dispatcher no longer accepts fails it too, so a rename cannot leave
stale coverage behind. A method that is not modelled says `unproven` here **and** is named in
[REPORT.md](REPORT.md)'s unproven list — an exemption declared where a reader looks (D-362). The spec names the
module the method's rule is checked in; the property-by-property anchors above say which claim.

| Method | Spec |
|---|---|
| `create_instance` | `V2Control.tla` (instance creation and its bootstrap grant) |
| `spawn_instance` | `V2Control.tla` (creation) with `V2Task.tla` (the task and return path it registers) |
| `issue_grant` | `V2Grants.tla` |
| `revoke_grant` | `V2Grants.tla` (the cascade) with `V2Authority.tla` (the id a revoke names) |
| `reauthorize_operation` | `V2Authority.tla` (the dispatch re-check) |
| `capability_requests` | `V2Capabilities.tla` (pending capability surface) |
| `approve_capability` | `V2Capabilities.tla` (attenuated task-scoped grant) |
| `deny_capability` | `V2Capabilities.tla` (fail-closed cancellation) |
| `reset_instance` | `V2Control.tla` (the epoch reset) |
| `create_work` | `V2Task.tla` |
| `send_message` | `V2Inbox.tla` |
| `drain_inbox` | `V2Inbox.tla` |
| `delegate_task` | `V2Task.tla` |
| `start_task` | `V2Task.tla` |
| `complete_task` | `V2Task.tla` |
| `cancel_task` | `V2Task.tla` |
| `read_history` | `V2History.tla` |
| `read_search_tree` | `V2SearchTree.tla`: D-506 User/Leader authority, read-only behavior and stale reporting; daemon/CLI presentation remains code-tested |
| `apply_search_tree` | `V2SearchTree.tla`: D-506 current-revision admission, replay stability, structural guards, merge refs and history; arbitrary text, multi-op patch order and SQLite execution remain code-tested |
| `fire_timer` | `V2Wait.tla` |
| `blocked_report` | `V2Wait.tla` |
| `submit_input` | `V2Control.tla` (the inbound boundary); `V2InputQueue.tla` (verified snapshots, original revision and at-most-once consumption) |
| `edit_input` | `V2InputQueue.tla` (pending identity/revision guard and retained original) |
| `cancel_input` | `V2InputQueue.tla` (pending identity/revision guard, serialized against consumption) |
| `begin_request` | `V2Control.tla`; prepared work scope in `V2Work.tla` |
| `configure_work` | `V2Work.tla` models enabled work transitions; policy validation and storage migration are sample-tested, not formally proved |
| `observe_operation_stops` | `V2Work.tla` models observation before takeover; OS identity and journal validation are sample-tested, not formally proved |
| `record_attempt` | `V2Control.tla` |
| `begin_compression` | `V2Compress.tla` |
| `compress_context` | `V2Compress.tla` |
| `fail_compression` | `V2Compress.tla` |
| `import_response` | `V2Control.tla` |
| `dispatch_operation` | `V2Control.tla` and `V2Approval.tla` |
| `complete_operation` | `V2Control.tla` |
| `cancel_operation` | `V2Control.tla` |
| `approve` | `V2Approval.tla` |
| `begin_review` | `V2Review.tla`: pending lifecycle, parked phase and reservation; budget/deadline composition and driver retry suppression remain code-tested |
| `record_review_attempt` | `V2Review.tla`: close/release, exact known totals for priced complete/malformed responses, and unknown unpriced/transport/interrupted usage; actual provider payload parsing remains code-tested |
| `review_decision` | `V2Review.tla`: completed consistent review and self-update guards for automatic allow; finality/revision composition remains in V2Approval/V2Control and Rust tests |
| `set_instance_profile` | `V2ProfileSwitch.tla`: user-only model/effort switch at a READY/WAITING boundary and frozen request snapshot; native adapter fields and context windows remain Rust/provider-tested |
| `deny` | `V2Approval.tla` |
| `fail_request` | `V2Control.tla` |
| `cancel_request` | `V2Control.tla` |
| `interrupt_instance` | `V2Control.tla` (the user's cancel of one running request) |
| `fork_reset` | `V2Control.tla` (the control-plane invariants its post-state must satisfy) |
| `complete_work` | `V2Task.tla` |
| `require_checks` | `V2Checks.tla` |
| `register_check_runs` | `V2Checks.tla` |
| `register_verification` | `V2Checks.tla` |
| `repair_completion` | `V2Checks.tla` |
| `block_work` | `V2Checks.tla` |
| `cancel_work` | `V2Control.tla` |
| `deadline_warning` | `V2Control.tla` (the `Warn` action: the wrap-up note, at most one per work and only while the deadline is still ahead — D-410) |
| `work_checks_briefing` | `V2Checks.tla` (the `Brief` action: the work's required checks are stated at the start of a turn, at most once and only for a live work that requires something — D-418) |
| `close_completion` | `V2Checks.tla` |
| `artifact_abandon` | `V2Artifact.tla` |
| `set_lifecycle` | `V2Control.tla` (termination) with `V2Task.tla` (the task cascade) |
| `artifact_stage` | `V2Artifact.tla` |
| `artifact_publish` | `V2Artifact.tla` |
| `artifact_gc_claim` | `V2Artifact.tla` |
| `artifact_release_spent_owners` | `V2Artifact.tla` (`DropOwner` with `OwnerHoldsUntilItsSubjectIsFinished` and `NothingIsReleasedInsideTheWindow`, D-436) |
| `artifact_collect` | `V2Artifact.tla` |
| `prune_history` | `V2Retention.tla` |


## Current module and configuration inventory

Every current module is listed with its configurations and marked claims. A marked claim without a refuting
control or a documented type exception fails `tools/verification_catalogue.py`.

### `V2Approval.tla`

Configurations: `MC_approval.cfg`, `MC_approval_drops_the_expiry.cfg`, `MC_approval_keeps_pending_on_close.cfg`, `MC_approval_parks_without_a_row.cfg`, `MC_approval_rewrites_a_decision.cfg`, `MC_approval_unapproved.cfg`.

Marked claims: `ApprovalDecisionIsFinal`, `EffectImpliesApproved`, `NoLateEffect`, `ParkedHasAnApprovalRow`, `TerminalOperationHasNoPendingApproval`, `TypeOK`.

### `V2Artifact.tla`

Configurations: `MC_artifact.cfg`, `MC_artifact_claim_anyone.cfg`, `MC_artifact_collector_ghost.cfg`, `MC_artifact_delete_anyone.cfg`, `MC_artifact_gc_ignores_references.cfg`, `MC_artifact_live_unpersisted.cfg`, `MC_artifact_live_without_reference.cfg`, `MC_artifact_reference_to_staging.cfg`, `MC_artifact_reference_unpersisted.cfg`, `MC_artifact_release_deletes_bytes.cfg`, `MC_artifact_release_fresh.cfg`, `MC_artifact_release_live_owner.cfg`.

Marked claims: `BytesOnlyDeletedWhileDeleting`, `ClaimOnlyFromLive`, `CollectorSkipsIncomplete`, `GcClaimsOnlyUnreferencedLive`, `LiveFlipCarriesReference`, `LiveIsPersisted`, `NoReferenceToUnpersisted`, `NothingIsReleasedInsideTheWindow`, `OwnerHoldsUntilItsSubjectIsFinished`, `ReferencesOnlyLive`, `ReleaseKeepsTheRowAndTheBytes`, `TypeOK`.

### `V2Authority.tla`

Configurations: `MC_authority.cfg`, `MC_authority_badview.cfg`, `MC_authority_cascade_over_reaches.cfg`, `MC_authority_dead_pair.cfg`, `MC_authority_extra_bootstrap.cfg`, `MC_authority_ghost_grant.cfg`, `MC_authority_list_ghost.cfg`, `MC_authority_orphan_child.cfg`, `MC_authority_revive_on_revoke.cfg`, `MC_authority_spawn_delegable.cfg`, `MC_authority_stale_effect.cfg`, `MC_authority_stalesurface.cfg`, `MC_authority_surface_foreign.cfg`, `MC_authority_task_child_delegable.cfg`, `MC_authority_trustsurface.cfg`, `MC_authority_two_effects.cfg`, `MC_authority_wider_child.cfg`.

Marked claims: `AuthorityTracesToTheUser`, `AuthorizedEffectsOnly`, `BootstrappedAuthority`, `CascadeOnlyTakesTheSubtree`, `CascadeTakesTheSubtree`, `ChildGrantsAreCoveredByTheirParent`, `DelegableGrantsAreUserIssued`, `EffectAtMostOnce`, `EveryLiveGrantBecomesRevocable`, `ListedIdsAreUsed`, `NoDeadGrantPair`, `OnceStaleNeverExecutes`, `RevokedStaysRevoked`, `StaleSurfaceCatchesUp`, `SurfaceChangesOnlyToTheCurrentEntitlement`, `TaskScopedGrantsAreNonDelegable`, `TypeOK`.

### `V2Capabilities.tla`

Configurations: `MC_capabilities.cfg`, `MC_capabilities_bad.cfg`, `MC_capabilities_delegable_child.cfg`, `MC_capabilities_revoke_keeps_child.cfg`, `MC_capabilities_settle_keeps_child.cfg`.

Marked claims: `ChildNonDelegable`, `NoChildWithoutParent`, `ParentRevokeCascades`, `TaskSettleRevokes`.

### `V2Checks.tla`

Configurations: `MC_checks.cfg`, `MC_checks_block_without_cause.cfg`, `MC_checks_brief_twice.cfg`, `MC_checks_brief_without.cfg`, `MC_checks_briefing.cfg`, `MC_checks_independent.cfg`, `MC_checks_late_join.cfg`, `MC_checks_late_require.cfg`, `MC_checks_rewinds_rounds.cfg`, `MC_checks_round_without_claim.cfg`, `MC_checks_self_verified.cfg`, `MC_checks_unbounded_rounds.cfg`, `MC_checks_unverified_accept.cfg`, `MC_checks_upgrade_candidate.cfg`.

Marked claims: `BlockedAfterTheBudgetOrStale`, `BriefingIsForAWorkWithChecks`, `BriefingIsOnce`, `ChecksOnlyJoinBeforeAClaim`, `ChecksOnlyVerifyAClaimedSuccess`, `EveryRequiredCheckWasVerified`, `IndependenceIsNotSelfVerified`, `NoUnverifiedSuccess`, `NoUpgradeOfTheCandidate`, `RoundsAreBounded`, `RoundsAreMonotone`, `TypeOK`.

### `V2Codemode.tla`

Configurations: `MC_codemode.cfg`, `MC_codemode_leak.cfg`, `MC_codemode_silent.cfg`, `MC_codemode_veto.cfg`.

Marked claims: `OnlyScriptOutputEnters`, `ScriptsAlwaysEmit`, `TypeOK`, `VetoedNeverCalled`.

### `V2Compress.tla`

Configurations: `MC_compress.cfg`, `MC_compress_append_in_hole.cfg`, `MC_compress_cover_backwards.cfg`, `MC_compress_cover_self.cfg`, `MC_compress_cover_to_non_summary.cfg`, `MC_compress_deletes_originals.cfg`, `MC_compress_keep_reservation.cfg`, `MC_compress_lifts_coverage.cfg`, `MC_compress_rewrites_closed.cfg`.

Marked claims: `ClosedCompressionReleasesReservation`, `CoverageNeverLifted`, `CoveragePointsForward`, `CoveredStaysCoveredByItsSummary`, `NewestSummaryIsVisible`, `NoEntryIsEverLost`, `RequestClosesOnce`, `TailAppend`, `TypeOK`.

### `V2Concurrency.tla`

Configurations: `MC_concurrency.cfg`, `MC_concurrency_reads_ahead.cfg`, `MC_concurrency_skips_an_event.cfg`, `MC_concurrency_writer_waits.cfg`.

Marked claims: `NoReaderSeesUncommitted`, `ReadersSeeTheCommittedPrefix`, `TypeOK`, `WriterProgressesDespiteAStalledReader`.

### `V2Control.tla`

Configurations: `MC.cfg`, `MC_control_BadActiveRequest.cfg`, `MC_control_BadAdmission.cfg`, `MC_control_CrossEpochReceipt.cfg`, `MC_control_DoubleEffect.cfg`, `MC_control_EffectAfterCancel.cfg`, `MC_control_EffectBeforeDispatch.cfg`, `MC_control_EffectOnTerminated.cfg`, `MC_control_EffectUnauthorized.cfg`, `MC_control_KeepClosedReservation.cfg`, `MC_control_OverReserve.cfg`, `MC_control_PreparedEffect.cfg`, `MC_control_RewriteTerminalOp.cfg`, `MC_control_RewriteTerminalWork.cfg`, `MC_control_SelectIncomplete.cfg`, `MC_control_StaleExecutor.cfg`, `MC_control_attempts.cfg`, `MC_control_cancel.cfg`, `MC_control_deadline.cfg`, `MC_control_decisions.cfg`, `MC_control_landing.cfg`, `MC_control_midturninput.cfg`, `MC_control_reask.cfg`, `MC_control_runtimeTail.cfg`, `MC_control_two.cfg`, `MC_control_two_disjunction.cfg`, `MC_control_warn.cfg`, `MC_control_warn_late.cfg`, `MC_control_warn_repeat.cfg`, `MC_wide.cfg`.

Marked claims: `AdmissionGate`, `AtMostOneWarning`, `CancelledBeforeStartHasNoEffect`, `CancelledWorkReleasesParkedInstances`, `EffectAtMostOnce`, `InputLandsAtTheBoundary`, `NoDispatchOnTerminated`, `NoEffectBeforeApproval`, `NoReceiptAcrossEpochs`, `NoRequestAfterDeadline`, `NoTurnWithoutWork`, `OneActiveRequest`, `PreparedIsNotTerminal`, `QueuedInputEntersTheContext`, `RecordBeforeEffect`, `ReservationReleased`, `ReservationsAdmitted`, `SelectionIsComplete`, `SettlementFollowsATurnAfterTheLanding`, `StaleExecutorRejected`, `TerminalOpStable`, `TerminalWorkStatusStable`, `TypeOK`, `WarnIsTimely`.

### `V2Coordinator.tla`

Configurations: `MC_coordinator.cfg`, `MC_coordinator_inherit.cfg`, `MC_coordinator_never_recovers.cfg`, `MC_coordinator_report.cfg`, `MC_coordinator_shared.cfg`, `MC_coordinator_sweep_beside_a_session.cfg`.

Marked claims: `AtMostOneCoordinator`, `NoHeldLockWithoutAHolder`, `RecoveryIsLive`, `SweepWaitsForTheSession`, `TheWindowIsNotMistakenForAHolder`, `TypeOK`.

### `V2Daemon.tla`

Configurations: `MC_daemon.cfg`, `MC_daemon_apply_on_another_version.cfg`, `MC_daemon_apply_without_receipt.cfg`, `MC_daemon_cursor_ahead.cfg`, `MC_daemon_delivers_twice.cfg`, `MC_daemon_duplicate_effect.cfg`, `MC_daemon_keeps_serving_after_receipt.cfg`, `MC_daemon_receipt_beyond_log.cfg`, `MC_daemon_reclaims_events.cfg`, `MC_daemon_rewrites_receipt.cfg`, `MC_daemon_rolls_back_log.cfg`, `MC_daemon_skips_an_event.cfg`, `MC_daemon_split_checkpoint.cfg`, `MC_daemon_stop.cfg`, `MC_daemon_stop_before_receipt.cfg`, `MC_daemon_view_from_snapshot.cfg`.

Marked claims: `AStopIsAnsweredAndEndsTheSession`, `AppliedAtMostOnce`, `AppliedCommandsUsedTheWireVersion`, `CommandApplicationCountIsBounded`, `CursorNeverBeyondLog`, `HandoversAreTheFullRange`, `LogMonotone`, `NoResyncInThisVersion`, `NothingIsServedTwice`, `ReceiptNamesARealVersion`, `ReceiptsAreStable`, `SnapshotNeverLeadsCursor`, `StopsOnlyAfterAnswering`, `TypeOK`, `ViewMatchesCursor`.

### `V2Discovery.tla`

Configurations: `MC_discovery.cfg`, `MC_discovery_cleanup.cfg`, `MC_discovery_pressure.cfg`.

Marked claims: `DiscoverySurvivesBackpressure`, `ExitedCoordinatorNotifiesDrivers`, `TypeOK`.

### `V2DiskFull.tla`

Configurations: `MC_diskfull.cfg`, `MC_diskfull_clear_anyway.cfg`, `MC_diskfull_fake_success.cfg`, `MC_diskfull_keep_driving.cfg`, `MC_diskfull_never_park.cfg`, `MC_diskfull_park_silently.cfg`.

Marked claims: `LatchClearsOnlyWhenWritable`, `LossIsReported`, `NoFakedSuccess`, `NoStepRunsWhileLatched`, `ParkEventuallyLands`, `TypeOK`.

### `V2DisplayWorker.tla`

Configurations: `MC_displayworker.cfg`, `MC_displayworker_apply.cfg`, `MC_displayworker_deadline.cfg`, `MC_displayworker_drop.cfg`, `MC_displayworker_leak.cfg`, `MC_displayworker_overflow.cfg`, `MC_displayworker_stale.cfg`.

Marked claims: `CurrentResultOnly`, `CurrentStartOnly`, `NoOrphanProcess`, `ObservedStopReaps`, `OwnerDropReaps`, `TypeOK`, `WorkerOutputBound`.

### `V2ExecutionChain.tla`

Configurations: `MC_executionchain.cfg`, `MC_executionchain_accept.cfg`, `MC_executionchain_commit.cfg`, `MC_executionchain_epoch.cfg`, `MC_executionchain_receipt.cfg`, `MC_executionchain_replay.cfg`, `MC_executionchain_stale.cfg`, `MC_executionchain_takeover.cfg`, `MC_executionchain_veto.cfg`.

Marked claims: `ContextKeepsItsEpoch`, `DispatchWasAuthorized`, `EffectHasControlCommit`, `EffectHasDurableAcceptance`, `TakeoverNeedsStopEvidence`, `TerminalReceiptIsImmutable`, `TypeOK`, `UnknownIsNeverReplayed`, `VetoPreventsEffect`.

### `V2ExportSnapshot.tla`

Configurations: `MC_exportsnapshot.cfg`, `MC_exportsnapshot_publish.cfg`, `MC_exportsnapshot_replace.cfg`, `MC_exportsnapshot_size.cfg`, `MC_exportsnapshot_snapshot.cfg`.

Marked claims: `ExistingDestinationSurvives`, `ExportBytesAreBounded`, `PagesShareSnapshot`, `PublicationIsComplete`, `TypeOK`.

### `V2Grants.tla`

Configurations: `MC_grants.cfg`, `MC_grants_bootstrap_extra.cfg`, `MC_grants_cascade_keeps_children.cfg`, `MC_grants_effect_twice.cfg`, `MC_grants_instance_delegable.cfg`, `MC_grants_mint_beyond_parent.cfg`, `MC_grants_revoke_keeps_live.cfg`, `MC_grants_runtime_issues.cfg`, `MC_grants_stale_dispatch.cfg`, `MC_grants_stale_offered_surface.cfg`, `MC_grants_task.cfg`, `MC_grants_task_child_delegable.cfg`, `MC_grants_task_child_orphan.cfg`, `MC_grants_unauthorized_effect.cfg`.

Marked claims: `AuthorityTracesToTheUser`, `AuthorizedEffectsOnly`, `BootstrappedAuthority`, `CascadeTakesTheSubtree`, `ChildGrantsAreCoveredByTheirParent`, `DelegableGrantsAreUserIssued`, `EffectAtMostOnce`, `OfferedToolsAreAuthorized`, `OnceStaleNeverExecutes`, `RevokedStaysRevoked`, `TaskScopedGrantsAreNonDelegable`, `TaskScopedGrantsHaveParents`, `TypeOK`, `TypeOKGrants`, `TypeOKOffered`, `TypeOKOps`, `TypeOKRevision`.

### `V2History.tla`

Configurations: `MC_history.cfg`, `MC_history_leaks.cfg`, `MC_history_system_reads.cfg`.

Marked claims: `InstancesReadOnlyTheirOwn`, `SystemNeverReads`, `TypeOK`.

### `V2Images.tla`

Configurations: `MC_images.cfg`, `MC_images_blind_model.cfg`, `MC_images_blind_parts.cfg`, `MC_images_leaked.cfg`, `MC_images_parts_from_loaded.cfg`, `MC_images_unbacked.cfg`, `MC_images_unreadable_ok.cfg`.

Marked claims: `NoImagesWithoutSupport`, `NoLeakedImages`, `NoUnbackedParts`, `PartsOnlyFromLoadedBytes`, `TypeOK`, `UnreadableIsReported`.

### `V2Inbox.tla`

Configurations: `MC_inbox.cfg`, `MC_inbox_drop_when_full.cfg`, `MC_inbox_foreign_drain.cfg`, `MC_inbox_no_dedup.cfg`, `MC_inbox_out_of_order.cfg`, `MC_inbox_stale_applied.cfg`, `MC_inbox_unbounded.cfg`.

Marked claims: `AtMostOncePerEnvelope`, `BoundedInbox`, `DrainIsOwned`, `LogIsIncreasing`, `NoSilentLoss`, `NoStaleApplication`, `TypeOK`.

### `V2InputQueue.tla`

Configurations: `MC_inputqueue.cfg`, `MC_inputqueue_original.cfg`, `MC_inputqueue_replay.cfg`, `MC_inputqueue_revision.cfg`, `MC_inputqueue_settled.cfg`, `MC_inputqueue_snapshot.cfg`, `MC_inputqueue_unverified.cfg`.

Marked claims: `AtMostOnceDelivery`, `OriginalRetained`, `PendingRevisionOnly`, `SnapshotDelivery`, `TypeOK`, `VerifiedAdmission`.

### `V2Isolation.tla`

Configurations: `MC_isolation.cfg`, `MC_isolation_fallback.cfg`, `MC_isolation_ssh.cfg`, `MC_isolation_ssh_fallback.cfg`, `MC_isolation_ssh_unreachable.cfg`, `MC_isolation_unavailable.cfg`.

Marked claims: `RunsOnlyUnderTheConfiguredBackend`, `TypeOK`, `UnavailableBackendNeverRuns`.

This is abstract isolation material only; it supplies no current host-sandbox guarantee.

### `V2JobSupervision.tla`

Configurations: `MC_jobsupervision.cfg`, `MC_jobsupervision_completion.cfg`, `MC_jobsupervision_grace.cfg`, `MC_jobsupervision_normal.cfg`, `MC_jobsupervision_persist.cfg`, `MC_jobsupervision_reap.cfg`, `MC_jobsupervision_retire.cfg`, `MC_jobsupervision_socket.cfg`, `MC_jobsupervision_timer.cfg`, `MC_jobsupervision_timestamp.cfg`.

Marked claims: `CancelGraceNeverReplenished`, `CancelledGroupWasEscalated`, `DeadlineEventuallyCancels`, `NormalServicesArePreserved`, `SettledRunnerEventuallyLeaves`, `TerminalHasRetirementTime`, `TypeOK`, `UnsavedAcceptanceNeverExecutes`.

### `V2Jobs.tla`

Configurations: `MC_jobs.cfg`, `MC_jobs_double_go.cfg`, `MC_jobs_effect_before_accept.cfg`, `MC_jobs_guess_not_run_effect.cfg`, `MC_jobs_guess_notrun.cfg`, `MC_jobs_keep_runner.cfg`, `MC_jobs_late_go.cfg`, `MC_jobs_retires_a_running_command.cfg`, `MC_jobs_service_signals_a_live_command.cfg`, `MC_jobs_service_signals_a_stranger.cfg`, `MC_jobs_spawn_first.cfg`.

Marked claims: `AtMostOneExecutor`, `EffectImpliesAcceptedStart`, `LateGoIsRejected`, `NoEffectBeforeAccept`, `NoLiveCommandWasSignalled`, `NotRunMeansNoEffect`, `NothingInFlightWasRetired`, `OnlyTheJobsOwnGroupIsSignalled`, `SettledRunnerLeaves`, `TypeOK`, `UnverifiableStartIsNeverGuessedNotRun`.

### `V2Memory.tla`

Configurations: `MC_memory.cfg`, `MC_memory_accept_beyond_cap.cfg`, `MC_memory_evicts.cfg`, `MC_memory_recall_beyond_cap.cfg`.

Marked claims: `NotesAreAppendOnly`, `RecallIsBounded`, `TheStoreRefusesAtTheCap`, `TypeOK`.

### `V2Oauth.tla`

Configurations: `MC_oauth.cfg`, `MC_oauth_logout_ignored.cfg`, `MC_oauth_reuses_code.cfg`, `MC_oauth_uses_expired.cfg`, `MC_oauth_verifier_leaks.cfg`, `MC_oauth_wrong_resource.cfg`.

Marked claims: `CodesAreSingleUse`, `LogoutIsEffective`, `NoRequestWithAnExpiredToken`, `TokenOnlyToItsResource`, `TypeOK`, `VerifierNeverLeavesTheClient`.

### `V2Presentation.tla`

Configurations: `MC_presentation.cfg`, `MC_presentation_dispatch.cfg`, `MC_presentation_eager.cfg`, `MC_presentation_erase.cfg`, `MC_presentation_overflow.cfg`, `MC_presentation_stale.cfg`.

Marked claims: `CacheIsBounded`, `CancelKeepsDraft`, `CompletionOnlyEdits`, `OnlyVisibleAdmitted`, `StaleResultIgnored`, `TypeOK`.

### `V2ProfileSwitch.tla`

Configurations: `MC_profileswitch.cfg`, `MC_profileswitch_midturn.cfg`, `MC_profileswitch_nonuser.cfg`, `MC_profileswitch_rewrite.cfg`, `MC_profileswitch_terminated.cfg`.

Marked claims: `OnlyUserSwitches`, `RequestProfileIsFrozen`, `SwitchUsesSafeBoundary`, `TypeOK`.

### `V2ProjectTrust.tla`

Configurations: `MC_projecttrust.cfg`, `MC_projecttrust_project_grants.cfg`, `MC_projecttrust_raw.cfg`, `MC_projecttrust_stale.cfg`.

Marked claims: `ListedTrustIsCanonical`, `TrustIsForTheRootTheUserListed`, `TypeOK`.

### `V2Prompt.tla`

Configurations: `MC_prompt.cfg`, `MC_prompt_ignore_unreadable.cfg`, `MC_prompt_members_use_their_profile.cfg`, `MC_prompt_rules_frozen_at_start.cfg`.

Marked claims: `EverybodyHasTheRules`, `PromptsFollowTheCurrentRules`, `TypeOK`, `UnreadableRulesAreNoted`.

### `V2Prune.tla`

Configurations: `MC_prune.cfg`, `MC_prune_dating.cfg`, `MC_prune_evidence.cfg`, `MC_prune_keep_present.cfg`, `MC_prune_live.cfg`, `MC_prune_window.cfg`.

Marked claims: `NothingThatShouldStayLeaves`, `TakenIsGone`, `TypeOK`.

### `V2Retention.tla`

Configurations: `MC_retention.cfg`, `MC_retention_evicts_evidence.cfg`, `MC_retention_evicts_live.cfg`, `MC_retention_evicts_young.cfg`, `MC_retention_ignores_live_only.cfg`, `MC_retention_runs_disabled.cfg`.

Marked claims: `EvictionOnlyUnderTheGuards`, `EvidenceIsNeverEvicted`, `NoReferenceToEvictedFact`, `OnlyOldFactsAreEvicted`, `TypeOK`.

### `V2Retry.tla`

Configurations: `MC_retry.cfg`, `MC_retry_off_by_one.cfg`, `MC_retry_parks_early.cfg`, `MC_retry_session_constant.cfg`.

Marked claims: `NoAttemptBeyondTheBudget`, `ParkOnlyAfterTheBudget`, `ResolvedIsTheInstancesOwn`, `TypeOK`, `ZeroMeansOneAttempt`.

### `V2Review.tla`

Configurations: `MC_review.cfg`, `MC_review_inconsistent.cfg`, `MC_review_phase.cfg`, `MC_review_reservation.cfg`, `MC_review_self_update.cfg`, `MC_review_unreviewed.cfg`, `MC_review_usage.cfg`.

Marked claims: `AllowRequiresCompletedReview`, `AllowRequiresConsistentAssessment`, `ClosedReviewReleasesReservation`, `FailedKnownUsageIsSettled`, `ReviewLeavesInstanceParked`, `SelfUpdateRemainsUserOnly`, `TypeOK`.

### `V2Schedule.tla`

Configurations: `MC_schedule.cfg`, `MC_schedule_disabled.cfg`, `MC_schedule_duplicate.cfg`, `MC_schedule_early.cfg`, `MC_schedule_overlap.cfg`.

Marked claims: `AtMostOneRunPerAutomation`, `DisabledNeverRuns`, `NoDuplicateRun`, `OnlyDueSlotsRun`, `TypeOK`.

### `V2SearchTree.tla`

Configurations: `MC_search_tree.cfg`, `MC_search_tree_add.cfg`, `MC_search_tree_apply_position.cfg`, `MC_search_tree_compact_error_flag.cfg`, `MC_search_tree_compact_no_advance.cfg`, `MC_search_tree_cycle.cfg`, `MC_search_tree_delete_history.cfg`, `MC_search_tree_drop_summary.cfg`, `MC_search_tree_focus.cfg`, `MC_search_tree_member_read.cfg`, `MC_search_tree_member_write.cfg`, `MC_search_tree_merge.cfg`, `MC_search_tree_merge_refs.cfg`, `MC_search_tree_merge_structure.cfg`, `MC_search_tree_move.cfg`, `MC_search_tree_node_limit.cfg`, `MC_search_tree_pair.cfg`, `MC_search_tree_pair_atomic.cfg`, `MC_search_tree_pair_replay.cfg`, `MC_search_tree_parent_drop.cfg`, `MC_search_tree_partial_compact.cfg`, `MC_search_tree_read_stale.cfg`, `MC_search_tree_read_writes.cfg`, `MC_search_tree_rejected_revision.cfg`, `MC_search_tree_replay.cfg`, `MC_search_tree_root_delete.cfg`, `MC_search_tree_source.cfg`, `MC_search_tree_stale.cfg`, `MC_search_tree_update.cfg`.

Marked claims: `AcceptedApplyRefreshesSource`, `Acyclic`, `ApplyPreservesPosition`, `CompactionAlwaysCommitsSummary`, `FailedCompactionMarksError`, `FailedCompactionPreservesTree`, `FocusExists`, `FreshApplyRequiresCurrentRevision`, `MergePreservesRefs`, `MergePreservesStructure`, `NodeLimit`, `OnlyAuthorizedReads`, `OnlyAuthorizedWrites`, `ParentsExist`, `ReadDoesNotChangeTree`, `ReadReportsStale`, `RejectedApplyPreservesTree`, `ReplayDoesNotGrow`, `RevisionHistoryComplete`, `RootPresent`, `TypeOK`, `ValidCompactionAdvancesTree`.

### `V2SessionUi.tla`

Configurations: `MC_sessionui.cfg`, `MC_sessionui_confirm.cfg`, `MC_sessionui_earlyexit.cfg`, `MC_sessionui_keepopen.cfg`, `MC_sessionui_switch.cfg`.

Marked claims: `ConfirmedDelete`, `DeletedCurrentExits`, `ExitAfterSuccess`, `SwitchAfterSuccess`, `TypeOK`.

### `V2Sessions.tla`

Configurations: `MC_sessions.cfg`, `MC_sessions_archive_default.cfg`, `MC_sessions_archive_held.cfg`, `MC_sessions_archive_while_held.cfg`, `MC_sessions_shared_path.cfg`.

Marked claims: `ArchiveOnlyWhenIdle`, `DefaultIsNeverArchived`, `NoCoordinatorForAnArchivedSession`, `RegistryIsInjective`, `TypeOK`.

### `V2SkillAuthority.tla`

Configurations: `MC_skillauthority.cfg`, `MC_skillauthority_escape.cfg`, `MC_skillauthority_grant.cfg`, `MC_skillauthority_unbound.cfg`.

Marked claims: `SkillContentCannotGrant`, `SkillReadNeedsBinding`, `SkillReadStaysInRegistry`, `TypeOK`.

### `V2Store.tla`

Configurations: `MC_store.cfg`, `MC_store_accept_unstamped.cfg`, `MC_store_adopt.cfg`, `MC_store_foreign_stamp.cfg`, `MC_store_interrupt_foreign.cfg`, `MC_store_refuse_and_write.cfg`, `MC_store_strand_interrupted.cfg`.

Marked claims: `AcceptedMeansStamped`, `EveryRefusalIsSilent`, `HalfInitializedIsCompleted`, `InterruptedWroteOursOnly`, `NoForeignAdoption`, `NoForeignStamp`, `NoForeignStampAlways`, `RefusalIsSilent`, `RefusalsWriteNothing`, `TypeOK`.

### `V2StreamBoundary.tla`

Configurations: `MC_streamboundary.cfg`, `MC_streamboundary_invalid.cfg`, `MC_streamboundary_partial.cfg`, `MC_streamboundary_retry.cfg`, `MC_streamboundary_size.cfg`, `MC_streamboundary_stall.cfg`, `MC_streamboundary_visible.cfg`.

Marked claims: `BodyIsBounded`, `InvalidEventsAreRejected`, `OnlyCompleteResponsesExecute`, `PartialChunksDoNotRenew`, `RetryBudgetIsBounded`, `TypeOK`, `VisibleTextIsNotReplayed`.

### `V2Task.tla`

Configurations: `MC_task.cfg`, `MC_task_bills_settled.cfg`, `MC_task_delegates_to_settled.cfg`, `MC_task_return_path_closed.cfg`, `MC_task_rewrite_settled.cfg`, `MC_task_self_dependency.cfg`, `MC_task_stale_active_work.cfg`, `MC_task_stranger_writes.cfg`, `MC_task_system_settles.cfg`, `MC_task_terminate_leaves_tasks.cfg`, `MC_task_two.cfg`, `MC_task_unordered_dependency.cfg`.

Marked claims: `DependenciesPointBackwards`, `NoOpenTaskOnDeadAssignee`, `NoSelfDependency`, `NoStaleActiveWork`, `OnlyPartiesWriteTasks`, `RegisteredWorkNeedsAnActiveWork`, `RequestsResolveToActiveWorks`, `ReturnPathOnlyWhileOpen`, `SettledIsFinal`, `SystemOnlyParksTasks`, `TypeOK`.

### `V2Trust.tla`

Configurations: `MC_trust.cfg`, `MC_trust_models_always_merge.cfg`, `MC_trust_project_grants_itself.cfg`, `MC_trust_project_overrides_user.cfg`, `MC_trust_project_sets_policy.cfg`, `MC_trust_refuses_silently.cfg`.

Marked claims: `NothingFromTheProjectUntrusted`, `PolicyClassesStayTheUsers`, `RefusalsAreNamed`, `TrustOnlyFromTheUser`, `TypeOK`, `UserDefinitionsNeverOverridden`.

### `V2Tui.tla`

Configurations: `MC_tui.cfg`, `MC_tui_approval.cfg`, `MC_tui_exit.cfg`, `MC_tui_target.cfg`.

Marked claims: `ExitOnlyDetaches`, `ExplicitDecision`, `LeaderOnly`, `TypeOK`.

### `V2Update.tla`

Configurations: `MC_update.cfg`, `MC_update_finish_anyway.cfg`, `MC_update_fresh_install.cfg`, `MC_update_ignore_lock.cfg`, `MC_update_keep_both_active.cfg`, `MC_update_pre_switch_crash.cfg`, `MC_update_rewrite_journal.cfg`, `MC_update_rollback_crash.cfg`, `MC_update_skip_validation.cfg`.

Marked claims: `AtMostOneUpdater`, `ExactlyOneActiveVersion`, `FailedHealthNeverCompletes`, `InterruptedRollbackSettlesToThePreviousVersion`, `PreSwitchCrashKeepsTheOldVersion`, `RevisionOnlyGrows`, `RollbackNeedsAPrevious`, `SwitchOnlyAfterValidation`, `TypeOK`.

### `V2Wait.tla`

Configurations: `MC_wait.cfg`, `MC_wait_answer_twice.cfg`, `MC_wait_answer_unresolved.cfg`, `MC_wait_answer_unused_slot.cfg`, `MC_wait_close_without_holding.cfg`, `MC_wait_closes_without_answering.cfg`, `MC_wait_deferred.cfg`, `MC_wait_mismatch_answer_call.cfg`, `MC_wait_never_sweep.cfg`, `MC_wait_park_after_answer.cfg`, `MC_wait_pending_while_ready.cfg`, `MC_wait_wakes_on_idle.cfg`, `MC_wait_wakes_on_report.cfg`, `MC_wait_widens_a_message.cfg`.

Marked claims: `AnswerImpliesResolved`, `IdleIsNotASettlement`, `MessageStaysAMessage`, `NoStrandedPending`, `PendingImpliesParked`, `ReportsAreNotSettlements`, `ResolvedWaitIsAnswered`, `SatisfiedHoldsConditions`, `TypeOK`, `UnusedSlotHasNoAnswer`, `WaitingHasPendingWait`, `WakeAnswerAtMostOnce`, `WakeAnswersItsCall`.

### `V2Work.tla`

Configurations: `MC_work.cfg`, `MC_work_attention.cfg`, `MC_work_cancel_stop.cfg`, `MC_work_completion.cfg`, `MC_work_double_bill.cfg`, `MC_work_early_effect.cfg`, `MC_work_info_scope.cfg`, `MC_work_prepare.cfg`, `MC_work_restart.cfg`, `MC_work_rewrite.cfg`, `MC_work_stop.cfg`, `MC_work_unanswered.cfg`.

Marked claims: `AdmissionUsageTransfersOnce`, `AttentionIsNotSettlement`, `CompletionMatchesCurrentWork`, `EffectsRequireAdmission`, `InformationDoesNotCreateWork`, `PreparationMatchesCurrentWork`, `RecoveryRemainsBounded`, `ResolvedAttentionIsAnswered`, `StopSurvivesObservation`, `TakeoverRequiresObservation`, `TypeOK`, `UnknownOutcomeIsPreserved`.

### `V2Workspace.tla`

Configurations: `MC_workspace.cfg`, `MC_workspace_merges_a_dirty_checkout.cfg`, `MC_workspace_merges_mid_turn.cfg`, `MC_workspace_retires_with_work.cfg`.

Marked claims: `NoMergeWhileATurnRuns`, `RetirementNeverBuriesWork`, `TypeOK`, `UncommittedWorkIsNeverMerged`.

## Boundaries

No full Rust/SQLite refinement, whole-system composition, arbitrary actor/command/bootstrap/schema/fork/retention/
export composition, arbitrary-string capability algebra, provider/scheduler fairness or live-model navigation
quality is established by this inventory. The separate wide configuration and Policy B source-bound bidirectional
fair-trace equivalence remain open. `TypeOK` is tracked separately in [COVERAGE.md](COVERAGE.md).

<!-- maps: verification/REPORT.md reader=evidence -->
<!-- maps: verification/REQUIREMENTS.md reader=evidence -->
<!-- maps: verification/COVERAGE.md reader=evidence -->
