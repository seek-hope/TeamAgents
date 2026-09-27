------------------------------ MODULE V2DiskFull -------------------------------
(***************************************************************************)
(* The write-failure latch (§4.4, A31; D-131-era driver work). A step whose  *)
(* submit fails with `StorageFull` must not become a dispatch storm and must *)
(* not become a lie: the driver latches, stops running steps, and retries    *)
(* only the park until it lands; the park's reason names the in-flight       *)
(* persistence loss, and the user frees space and resumes. The rules the     *)
(* code and the documents state, and what this model checks: * while the     *)
(* latch is set, no step runs — the failed step is never retried and new     *)
(* side effects stay stopped (`NoStepRunsWhileLatched`); * success is never  *)
(* faked: a step whose write did not land is never reported as succeeded     *)
(* (`NoFakedSuccess`); * the uncertainty is preserved: when an effect was in *)
(* flight and its outcome could not be persisted, the park reports that loss *)
(* (`LossIsReported`) — the honest report, not a silent park and not a       *)
(* claimed outcome; * the latch clears only once a write lands               *)
(* (`LatchClearsOnlyWhenWritable` — the park itself is what clears it, which *)
(* is why a full store keeps the instance latched; * and the retry is live:  *)
(* once the store is writable again the park lands (`ParkEventuallyLands`,   *)
(* under strong fairness of the park step, whose promise is conditional: a single writable moment proves nothing, so the property says "while writability recurs": the environment may make the    *)
(* store writable only intermittently while the driver retries at poll pace  *)
(* forever, so weak fairness is not enough — `V2Control` uses                *)
(* `SF_vars(ApplyQueued(i))` for the same reason). Code anchors:             *)
(* engine/src/v2/driver.rs — `storage_full(error)` (the two signatures,      *)
(* core's "StorageFull: …" from the submit boundary and the OS's "No space   *)
(* left on device" from artifact/journal writes), the `storage_full` field's *)
(* comment ("once a submit fails StorageFull the driver …"), the latched     *)
(* branch that submits only the park — with the text "storage full: an       *)
(* in-flight result could not be persisted (§4.4); free space, then resume   *)
(* the instance" — clears the latch only when that submit is `ok`, and the   *)
(* `drive_once` arm that sets the latch on a `storage_full` error. The       *)
(* accepting end is                                                          *)
(* `control::disk_full_is_classified_at_the_submit_boundary` (a *real*       *)
(* `SQLITE_FULL` through the storage worker, not a mocked error) and         *)
(* `v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking`.  *)
(* What the model is and is not: one instance, one in-flight effect and its  *)
(* outcome — enough for the latch's rules and no more. It does not model the *)
(* *cost* side (an artifact write that fails also latches: the artifact and  *)
(* journal paths share the signature, which the model captures by treating   *)
(* "a write" as one thing) nor the storage worker's queueing, the operation  *)
(* ledger's own states after a lost outcome (that is `V2Control`'s           *)
(* crash/recovery and unknown-usage story), or the tool that produced the    *)
(* effect. `ParkEventuallyLands` needs the park step to be attempted while   *)
(* the store is writable — the code's poll-pace retry — which is what the    *)
(* strong fairness assumption says (weak fairness would let an environment   *)
(* that only intermittently frees space starve the retry); the user's resume *)
(* is a separate step here because the code makes it separate too. The four  *)
(* counterfactuals are the defects the rules exist against: `KeepDriving`    *)
(* (steps keep running while latched — the dispatch storm), `FakeSuccess` (a *)
(* step whose write failed is reported as succeeded), `ParkSilently` (the    *)
(* parking loses the in-flight loss), and `ClearAnyway` (the latch clears    *)
(* although nothing could be written).                                       *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS KeepDriving,   \* counterfactual: the driver keeps running steps while latched
          FakeSuccess,   \* counterfactual: a step whose write failed is reported as succeeded
          ParkSilently,  \* counterfactual: the park lands without naming the in-flight loss
          ClearAnyway    \* counterfactual: the latch clears although the store is still full

Stores == {"ok", "full"}
Lifecycles == {"active", "parked"}
Outcomes == {"none", "persisted", "lost"}
Reports == {"none", "unknown"}

VARIABLES
  store,          \* whether a write lands: ok | full
  latched,        \* the driver's storage-full latch
  lifecycle,      \* the instance's lifecycle: active | parked
  inFlight,       \* none | effect: a dispatched side effect whose outcome is not persisted yet
  outcome,        \* what became of that effect's outcome: none | persisted | lost
  reported,       \* none | unknown: whether the park named the in-flight loss
  ranWhileLatched, \* monitor: a step ran while the latch was set
  fakedSuccess,   \* monitor: a success was reported for a step whose write failed
  clearedWhileFull \* monitor: the latch cleared while the store was full

vars == <<store, latched, lifecycle, inFlight, outcome, reported>>
monVars == <<vars, ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* ------------------------------------------------------------------- actions --
\* The disk fills — at any moment, which is why a step can be in flight.
FillStore ==
  /\ store = "ok"
  /\ store' = "full"
  /\ UNCHANGED <<latched, lifecycle, inFlight, outcome, reported,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* The user frees space (which is what makes a resume possible).
FreeStore ==
  /\ store = "full"
  /\ store' = "ok"
  /\ UNCHANGED <<latched, lifecycle, inFlight, outcome, reported,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* A step starts its side effect: the driver is driving, the store looked fine.
RunStep ==
  /\ lifecycle = "active"
  /\ ~latched
  /\ inFlight = "none"
  /\ store = "ok"
  /\ inFlight' = "effect"
  /\ UNCHANGED <<store, latched, lifecycle, outcome, reported,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* The effect's outcome is persisted: the write landed.
PersistOutcome ==
  /\ inFlight = "effect"
  /\ store = "ok"
  /\ inFlight' = "none"
  /\ outcome' = "persisted"
  /\ UNCHANGED <<store, latched, lifecycle, reported,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* The write of that outcome fails (§4.4): the effect happened, its outcome is
\* lost, and the driver latches instead of retrying the step.
LoseOutcome ==
  /\ inFlight = "effect"
  /\ store = "full"
  /\ ~FakeSuccess
  /\ inFlight' = "none"
  /\ outcome' = "lost"
  /\ latched' = TRUE
  /\ UNCHANGED <<store, lifecycle, reported,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* The counterfactual: the same failed write is reported as a success.
\* (Written as "persisted" although nothing landed, which is what faking is.)
FakeTheOutcome ==
  /\ FakeSuccess
  /\ inFlight = "effect"
  /\ store = "full"
  /\ inFlight' = "none"
  /\ outcome' = "persisted"
  /\ latched' = TRUE
  /\ fakedSuccess' = TRUE
  /\ UNCHANGED <<store, lifecycle, reported,
                 ranWhileLatched, clearedWhileFull>>

\* The counterfactual of the latch: the driver keeps driving.
DriveWhileLatched ==
  /\ KeepDriving
  /\ latched
  /\ lifecycle = "active"
  /\ inFlight = "none"
  /\ inFlight' = "effect"
  /\ ranWhileLatched' = TRUE
  /\ UNCHANGED <<store, latched, lifecycle, outcome, reported,
                 fakedSuccess, clearedWhileFull>>

\* The park, retried at poll pace while the latch is set: it lands only when a
\* write can land, and it names the in-flight loss when there was one.
ParkWhileLatched ==
  /\ latched
  /\ lifecycle = "active"
  /\ inFlight = "none"
  /\ store = "ok"
  /\ lifecycle' = "parked"
  /\ latched' = FALSE
  /\ reported' = IF outcome = "lost" /\ ~ParkSilently THEN "unknown" ELSE reported
  /\ UNCHANGED <<store, inFlight, outcome,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

\* The same attempt while the store is still full: nothing lands, and the latch
\* stays — unless the counterfactual clears it anyway.
ParkAttemptWhileFull ==
  /\ latched
  /\ lifecycle = "active"
  /\ store = "full"
  /\ ~ClearAnyway
  /\ UNCHANGED monVars

ClearLatchAnyway ==
  /\ ClearAnyway
  /\ latched
  /\ lifecycle = "active"
  /\ store = "full"
  /\ latched' = FALSE
  /\ clearedWhileFull' = TRUE
  /\ UNCHANGED <<store, lifecycle, inFlight, outcome, reported,
                 ranWhileLatched, fakedSuccess>>

\* The user resumes the parked instance once the space is freed (§4.4).
UserResume ==
  /\ lifecycle = "parked"
  /\ lifecycle' = "active"
  /\ UNCHANGED <<store, latched, inFlight, outcome, reported,
                 ranWhileLatched, fakedSuccess, clearedWhileFull>>

Stutter == UNCHANGED monVars

Next ==
  \/ FillStore
  \/ FreeStore
  \/ RunStep
  \/ PersistOutcome
  \/ LoseOutcome
  \/ FakeTheOutcome
  \/ DriveWhileLatched
  \/ ParkWhileLatched
  \/ ParkAttemptWhileFull
  \/ ClearLatchAnyway
  \/ UserResume
  \/ Stutter

Init ==
  /\ store = "ok"
  /\ latched = FALSE
  /\ lifecycle = "active"
  /\ inFlight = "none"
  /\ outcome = "none"
  /\ reported = "none"
  /\ ranWhileLatched = FALSE
  /\ fakedSuccess = FALSE
  /\ clearedWhileFull = FALSE

\* Strong fairness on the park: the driver retries it at poll pace while latched
\* (the drive loop never stops), so as long as a write can land infinitely often
\* the latch does not stay set forever. Weak fairness would let an environment
\* that only intermittently frees space starve the retry, which is not the design.
Spec == Init /\ [][Next]_monVars /\ SF_monVars(ParkWhileLatched)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ store \in Stores
  /\ latched \in BOOLEAN
  /\ lifecycle \in Lifecycles
  /\ inFlight \in {"none", "effect"}
  /\ outcome \in Outcomes
  /\ reported \in Reports
  /\ ranWhileLatched \in BOOLEAN
  /\ fakedSuccess \in BOOLEAN
  /\ clearedWhileFull \in BOOLEAN

\* A31: while the latch is set no step runs — the failed step is never retried
\* and new side effects stay stopped.
NoStepRunsWhileLatched == ranWhileLatched = FALSE

\* A31: success is never faked — a step whose write did not land is never
\* reported as succeeded.
NoFakedSuccess == fakedSuccess = FALSE

\* A31: the uncertainty is preserved — a parked instance whose in-flight outcome
\* was lost says so.
LossIsReported == (outcome = "lost" /\ lifecycle = "parked") => reported = "unknown"

\* The latch clears only because the park landed, which needs a write to land.
LatchClearsOnlyWhenWritable == clearedWhileFull = FALSE

\* --------------------------------------------------------------- properties --
\* §4.4: free space, and the latched instance parks. The condition has to be the
\* honest one: a single writable moment proves nothing (the disk can refill before
\* the next poll), so the promise is that the retry is live *while writability
\* recurs* — strong fairness of the park, against an environment that may fill the
\* disk again and again.
ParkEventuallyLands == []( (latched /\ []<>(store = "ok")) => <>(~latched) )

=============================================================================
