# Claims without a refuting control

`review/verification_catalogue.py` requires every claim a positive configuration checks to be
either the `expects:` target of a counterexample configuration (that control is then shown to
refute it, D-462) or a row here with a status. **105** claims are in this state today: they are
enumerated exhaustively by TLC, but no configuration is shown to break them, so a claim that
became vacuous would still verify. The number is recomputed and compared by the audit; it can only
go down by adding a control (and then deleting the row) or by replacing the row with a justified
status: `entailed-by: <Claim>` (the named claim is refuted by a control and implies this one) or
`structural: <reason>` (a property of the definition, not of a reachable state).

Regenerate with `python3 review/verification_catalogue.py --write-coverage`.

| Module | Claim | Status |
|---|---|---|
| `V2Approval` | `EffectImpliesApproved` | needs-control |
| `V2Artifact` | `BytesOnlyDeletedWhileDeleting` | needs-control |
| `V2Artifact` | `ClaimOnlyFromLive` | needs-control |
| `V2Artifact` | `CollectorSkipsIncomplete` | needs-control |
| `V2Artifact` | `LiveFlipCarriesReference` | needs-control |
| `V2Artifact` | `LiveIsPersisted` | needs-control |
| `V2Artifact` | `NoReferenceToUnpersisted` | needs-control |
| `V2Artifact` | `ReferencesOnlyLive` | needs-control |
| `V2Artifact` | `ReleaseKeepsTheRowAndTheBytes` | needs-control |
| `V2Authority` | `AuthorityTracesToTheUser` | needs-control |
| `V2Authority` | `BootstrappedAuthority` | needs-control |
| `V2Authority` | `CascadeOnlyTakesTheSubtree` | needs-control |
| `V2Authority` | `CascadeTakesTheSubtree` | needs-control |
| `V2Authority` | `ChildGrantsAreCoveredByTheirParent` | needs-control |
| `V2Authority` | `DelegableGrantsAreUserIssued` | needs-control |
| `V2Authority` | `EffectAtMostOnce` | needs-control |
| `V2Authority` | `ListedIdsAreUsed` | needs-control |
| `V2Authority` | `NoDeadGrantPair` | needs-control |
| `V2Authority` | `OnceStaleNeverExecutes` | needs-control |
| `V2Authority` | `RevokedStaysRevoked` | needs-control |
| `V2Authority` | `SurfaceChangesOnlyToTheCurrentEntitlement` | needs-control |
| `V2Authority` | `TaskScopedGrantsAreNonDelegable` | needs-control |
| `V2Checks` | `BlockedAfterTheBudgetOrStale` | needs-control |
| `V2Checks` | `BriefingIsOnce` | needs-control |
| `V2Checks` | `ChecksOnlyJoinBeforeAClaim` | needs-control |
| `V2Checks` | `ChecksOnlyVerifyAClaimedSuccess` | needs-control |
| `V2Checks` | `NoUnverifiedSuccess` | needs-control |
| `V2Checks` | `NoUpgradeOfTheCandidate` | needs-control |
| `V2Checks` | `RoundsAreBounded` | needs-control |
| `V2Codemode` | `ScriptsAlwaysEmit` | needs-control |
| `V2Compress` | `ClosedCompressionReleasesReservation` | needs-control |
| `V2Compress` | `CoveragePointsForward` | needs-control |
| `V2Compress` | `CoveredStaysCoveredByItsSummary` | needs-control |
| `V2Compress` | `NewestSummaryIsVisible` | needs-control |
| `V2Compress` | `TailAppend` | needs-control |
| `V2Control` | `AdmissionGate` | needs-control |
| `V2Control` | `CancelledBeforeStartHasNoEffect` | needs-control |
| `V2Control` | `EffectAtMostOnce` | needs-control |
| `V2Control` | `NoEffectBeforeApproval` | needs-control |
| `V2Control` | `NoEffectOnTerminated` | needs-control |
| `V2Control` | `NoReceiptAcrossEpochs` | needs-control |
| `V2Control` | `OneActiveRequest` | needs-control |
| `V2Control` | `PreparedIsNotTerminal` | needs-control |
| `V2Control` | `RecordBeforeEffect` | needs-control |
| `V2Control` | `ReservationReleased` | needs-control |
| `V2Control` | `ReservationsAdmitted` | needs-control |
| `V2Control` | `SelectionIsComplete` | needs-control |
| `V2Control` | `StaleExecutorRejected` | needs-control |
| `V2Control` | `TerminalGoalStatusStable` | needs-control |
| `V2Control` | `TerminalOpStable` | needs-control |
| `V2Coordinator` | `RecoveryIsLive` | needs-control |
| `V2Daemon` | `AppliedAtMostOnce` | needs-control |
| `V2Daemon` | `AppliedCommandsUsedTheWireVersion` | needs-control |
| `V2Daemon` | `CursorNeverBeyondLog` | needs-control |
| `V2Daemon` | `ReceiptNamesARealVersion` | needs-control |
| `V2Daemon` | `SnapshotNeverLeadsCursor` | needs-control |
| `V2Daemon` | `ViewMatchesCursor` | needs-control |
| `V2DiskFull` | `ParkEventuallyLands` | needs-control |
| `V2Grants` | `AuthorityTracesToTheUser` | needs-control |
| `V2Grants` | `AuthorizedEffectsOnly` | needs-control |
| `V2Grants` | `BootstrappedAuthority` | needs-control |
| `V2Grants` | `CascadeTakesTheSubtree` | needs-control |
| `V2Grants` | `ChildGrantsAreCoveredByTheirParent` | needs-control |
| `V2Grants` | `DelegableGrantsAreUserIssued` | needs-control |
| `V2Grants` | `EffectAtMostOnce` | needs-control |
| `V2Grants` | `OnceStaleNeverExecutes` | needs-control |
| `V2Grants` | `RevokedStaysRevoked` | needs-control |
| `V2Grants` | `TaskScopedGrantsAreNonDelegable` | needs-control |
| `V2Grants` | `TaskScopedGrantsHaveParents` | needs-control |
| `V2Images` | `NoLeakedImages` | needs-control |
| `V2Images` | `NoUnbackedParts` | needs-control |
| `V2Images` | `PartsOnlyFromLoadedBytes` | needs-control |
| `V2Images` | `UnreadableIsReported` | needs-control |
| `V2Inbox` | `LogIsIncreasing` | needs-control |
| `V2Jobs` | `EffectImpliesAcceptedStart` | needs-control |
| `V2Jobs` | `NotRunMeansNoEffect` | needs-control |
| `V2Jobs` | `SettledRunnerLeaves` | needs-control |
| `V2Memory` | `RecallIsBounded` | needs-control |
| `V2Memory` | `TheStoreRefusesAtTheCap` | needs-control |
| `V2Prune` | `TakenIsGone` | needs-control |
| `V2Retention` | `EvictionOnlyUnderTheGuards` | needs-control |
| `V2Retry` | `ZeroMeansOneAttempt` | needs-control |
| `V2Sessions` | `ArchiveOnlyWhenIdle` | needs-control |
| `V2Store` | `AcceptedMeansStamped` | needs-control |
| `V2Store` | `EveryRefusalIsSilent` | needs-control |
| `V2Store` | `HalfInitializedIsCompleted` | needs-control |
| `V2Store` | `InterruptedWroteOursOnly` | needs-control |
| `V2Store` | `NoForeignStamp` | needs-control |
| `V2Store` | `NoForeignStampAlways` | needs-control |
| `V2Store` | `RefusalIsSilent` | needs-control |
| `V2Store` | `RefusalsWriteNothing` | needs-control |
| `V2Task` | `NoSelfDependency` | needs-control |
| `V2Task` | `NoStaleActiveGoal` | needs-control |
| `V2Task` | `OnlyPartiesWriteTasks` | needs-control |
| `V2Task` | `ReturnPathOnlyWhileOpen` | needs-control |
| `V2Task` | `SettledIsFinal` | needs-control |
| `V2Task` | `SystemOnlyParksTasks` | needs-control |
| `V2Wait` | `AnswerImpliesResolved` | needs-control |
| `V2Wait` | `NoStrandedPending` | needs-control |
| `V2Wait` | `PendingImpliesParked` | needs-control |
| `V2Wait` | `SatisfiedHoldsConditions` | needs-control |
| `V2Wait` | `UnusedSlotHasNoAnswer` | needs-control |
| `V2Wait` | `WaitingHasPendingWait` | needs-control |
| `V2Wait` | `WakeAnswerAtMostOnce` | needs-control |
| `V2Wait` | `WakeAnswersItsCall` | needs-control |
