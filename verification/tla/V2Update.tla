------------------------------- MODULE V2Update -------------------------------
(***************************************************************************)
(* D-453 governed self-update, modelled for D-462: validate an artifact, stop *)
(* the daemon, switch the version pointer, start the new daemon, health-check *)
(* it and keep it or roll back. The journal is append-only and single-writer, *)
(* so a crash at any step leaves exactly one active version.                  *)
(*                                                                          *)
(* Code anchors: engine/src/update.rs — JournalLock (one writer),             *)
(* UpdateJournal::append (hash chain, monotone revision), settle (interrupted *)
(* / recovered / recovered_rollback), Layout::switch_to (atomic symlink),     *)
(* rollback, and staged → stopping → switched → completed | rolling_back →    *)
(* rolled_back as the health check decides.                                   *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Updaters, Versions, MaxRevision,
          IgnoreLock,        \* counterfactual: a second updater acquires too
          RewriteJournal,    \* counterfactual: an append rewinds the revision
          SkipValidation,    \* counterfactual: the switch runs with no validated artifact
          KeepBothActive,    \* counterfactual: the switch keeps the old version active
          FreshInstall,      \* counterfactual: the switch records no previous version
          CrashSwitches,     \* counterfactual: settling an interrupted journal moves the pointer
          RollbackAnyway,    \* counterfactual: rollback with nothing to restore
          FinishAnyway,      \* counterfactual: a failed health check still completes
          RecoverKeepsTheNewVersion \* counterfactual: an interrupted rollback keeps the new version

Phases == {"idle", "staged", "stopping", "switched", "rolling_back", "rolled_back", "completed"}
Health == {"unknown", "ok", "failed"}

VARIABLES
  phase,
  revision,
  holders,
  active,
  previous,
  artifact,
  health,
  rewound,
  unconverified,
  no_target,
  completed_failed

vars == <<phase, revision, holders, active, previous, artifact, health>>
monVars == <<vars, rewound, unconverified, no_target, completed_failed>>

Init ==
  /\ phase = "idle" /\ revision = 0 /\ holders = {}
  /\ active = {"v1"} /\ previous = "none" /\ artifact = FALSE /\ health = "unknown"
  /\ rewound = FALSE /\ unconverified = FALSE /\ no_target = FALSE
  /\ completed_failed = FALSE

\* The journal is single-writer: a second updater is refused while one holds it.
Acquire(u) ==
  /\ u \in Updaters /\ u \notin holders
  /\ (IgnoreLock \/ holders = {})
  /\ holders' = holders \cup {u}
  /\ UNCHANGED <<phase, revision, active, previous, artifact, health, rewound, unconverified,
                 no_target, completed_failed>>

\* Stage and validate the artifact before anything is stopped.
Stage ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ phase = "idle" /\ artifact = FALSE
  /\ phase' = "staged"
  /\ artifact' = TRUE
  /\ revision' = IF RewriteJournal /\ revision > 0 THEN revision - 1 ELSE revision + 1
  /\ rewound' = rewound \/ (RewriteJournal /\ revision > 0)
  /\ UNCHANGED <<holders, active, previous, health, unconverified, no_target, completed_failed>>

\* The daemon is stopped before the pointer moves (the counterfactual skips staging entirely).
Stop ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ phase = "staged"
  /\ phase' = "stopping"
  /\ revision' = revision + 1
  /\ UNCHANGED <<holders, active, previous, artifact, health, rewound, unconverified, no_target,
                 completed_failed>>

\* The switch: the new version becomes the one `current` names, the old one becomes `previous`.
Switch ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ phase = "stopping"
  /\ unconverified' = unconverified \/ ~artifact
  /\ phase' = "switched"
  /\ active' = IF KeepBothActive THEN active \cup {"v2"} ELSE {"v2"}
  /\ previous' = CHOOSE v \in active : TRUE
  /\ revision' = revision + 1
  /\ UNCHANGED <<holders, artifact, health, rewound, no_target, completed_failed>>

\* The new daemon answers its health check: ok completes the update, a failure starts a rollback.
HealthCheck(ok) ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ phase = "switched" /\ health = "unknown"
  /\ health' = IF ok THEN "ok" ELSE "failed"
  /\ phase' = IF ok THEN "completed" ELSE "rolling_back"
  /\ revision' = revision + 1
  /\ UNCHANGED <<holders, active, previous, artifact, rewound, unconverified, no_target,
                 completed_failed>>

\* A failed health check restores the complete previous version.
Rollback ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ phase = "rolling_back"
  /\ no_target' = no_target \/ (previous = "none")
  /\ phase' = "rolled_back"
  /\ active' = IF previous = "none" THEN active ELSE {previous}
  /\ previous' = "none"
  /\ revision' = revision + 1
  /\ UNCHANGED <<holders, artifact, health, rewound, unconverified, completed_failed>>

\* The counterfactual: the update reports completion although the health check failed.
FinishAnywayStep ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ FinishAnyway /\ phase = "rolling_back"
  /\ phase' = "completed"
  /\ completed_failed' = TRUE
  /\ revision' = revision + 1
  /\ UNCHANGED <<holders, active, previous, artifact, health, rewound, unconverified, no_target>>

\* A crash leaves the pointer where the last committed step left it.
Crash ==
  /\ phase \in {"staged", "stopping", "switched", "rolling_back"}
  /\ UNCHANGED monVars

\* A restart settles an interrupted journal: before the switch the old version stays active.
Recover ==
  /\ revision < MaxRevision  \* the journal is bounded so the check terminates
  /\ phase \in {"staged", "stopping"}
  /\ phase' = "idle" /\ artifact' = FALSE
  /\ active' = IF CrashSwitches THEN {"v2"} ELSE active
  /\ /\ revision' = revision + 1
  /\ UNCHANGED <<holders, previous, health, rewound, unconverified, no_target, completed_failed>>

Stutter == UNCHANGED monVars

\* D-462 counterfactuals: each is the mistake itself, one reachable step, so the claim it targets
\* is refuted by construction rather than through a chain of guards.
SwitchWithNoArtifact ==
  /\ SkipValidation /\ phase \in {"idle", "stopping"}
  /\ phase' = "switched" /\ active' = {"v2"}
  /\ unconverified' = TRUE
  /\ UNCHANGED <<revision, holders, previous, artifact, health, rewound, no_target, completed_failed>>

RollbackWithNoPrevious ==
  /\ RollbackAnyway /\ phase \in {"switched", "rolling_back"}
  /\ phase' = "rolled_back" /\ no_target' = TRUE
  /\ UNCHANGED <<revision, holders, active, previous, artifact, health, rewound, unconverified,
                 completed_failed>>

CompleteDespiteFailure ==
  /\ FinishAnyway /\ phase \in {"switched", "rolling_back"}
  /\ phase' = "completed" /\ completed_failed' = TRUE
  /\ UNCHANGED <<revision, holders, active, previous, artifact, health, rewound, unconverified,
                 no_target>>

RecordNoPrevious ==
  /\ FreshInstall /\ phase = "switched"
  /\ previous' = "none"
  /\ UNCHANGED <<phase, revision, holders, active, artifact, health, rewound, unconverified, no_target,
                 completed_failed>>

\* D-462: a crash during the rollback settles the previous version (the code's `recovered_rollback`).
RecoversRollback ==
  /\ phase = "rolling_back" /\ previous # "none"
  /\ phase' = "rolled_back"
  /\ active' = IF RecoverKeepsTheNewVersion THEN {"v2"} ELSE {previous}
  /\ revision' = revision + 1
  /\ UNCHANGED <<holders, previous, artifact, health, rewound, unconverified, no_target, completed_failed>>

Next ==
  \/ \E u \in Updaters : Acquire(u)
  \/ Stage \/ Stop \/ Switch \/ HealthCheck(TRUE) \/ HealthCheck(FALSE) \/ Rollback
  \/ SwitchWithNoArtifact \/ RollbackWithNoPrevious \/ CompleteDespiteFailure \/ RecordNoPrevious
  \/ Crash \/ Recover \/ RecoversRollback \/ Stutter

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ phase \in Phases /\ revision \in Nat /\ holders \subseteq Updaters
  /\ active \subseteq Versions /\ active # {}
  /\ previous \in Versions \union {"none"}
  /\ artifact \in BOOLEAN /\ health \in Health

AtMostOneUpdater == Cardinality(holders) <= 1
SwitchOnlyAfterValidation == ~unconverified
ExactlyOneActiveVersion == Cardinality(active) = 1
RollbackNeedsAPrevious == ~no_target
FailedHealthNeverCompletes == ~completed_failed
\* --------------------------------------------------------------- properties --
RevisionOnlyGrows == [][ revision' >= revision ]_monVars
PreSwitchCrashKeepsTheOldVersion ==
  [][ (phase \in {"staged", "stopping"} /\ phase' = "idle") => active' = active ]_monVars
InterruptedRollbackSettlesToThePreviousVersion ==
  [][ (phase = "rolling_back" /\ phase' = "rolled_back") => active' = {previous} ]_monVars

=============================================================================
