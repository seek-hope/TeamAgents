--------------------------- MODULE V2Discovery ---------------------------
(***************************************************************************)
(* D-494: a bounded storage queue must not silently end instance discovery. *)
(* Anchors: engine/src/v2/storage.rs::Storage::call and                      *)
(* engine/src/v2/supervisor.rs::Supervisor::{run,discover}.                  *)
(* A full queue rejects a read before submission; retrying that read cannot *)
(* replay a mutation. One finite saturation burst is abstracted here. After *)
(* it drains, a fair discovery pass starts the demanded driver, unless the  *)
(* user stops or a distinct fatal error is reported. The exit path notifies *)
(* existing drivers on both normal and error exits. This does not prove     *)
(* progress under permanent overload, scheduler fairness, successful driver *)
(* construction, provider termination, SQL correctness or Rust refinement.  *)
(***************************************************************************)
EXTENDS Naturals
CONSTANT Bug
VARIABLES
  full,
  saturated,
  running,
  demanded,
  discovered,
  stopped,
  fault,
  notified
vars == <<full, saturated, running, demanded, discovered, stopped, fault, notified>>
Init ==
  /\ full = FALSE
  /\ saturated = FALSE
  /\ running = TRUE
  /\ demanded = FALSE
  /\ discovered = FALSE
  /\ stopped = FALSE
  /\ fault = FALSE
  /\ notified = FALSE
Demand ==
  /\ running
  /\ ~demanded
  /\ demanded' = TRUE
  /\ UNCHANGED <<full, saturated, running, discovered, stopped, fault, notified>>
Saturate ==
  /\ running
  /\ ~saturated
  /\ full' = TRUE
  /\ saturated' = TRUE
  /\ UNCHANGED <<running, demanded, discovered, stopped, fault, notified>>
Drain ==
  /\ full
  /\ full' = FALSE
  /\ UNCHANGED <<saturated, running, demanded, discovered, stopped, fault, notified>>
Discover ==
  /\ running
  /\ IF full /\ Bug = "exit-full"
     THEN
          /\ running' = FALSE
          /\ notified' = TRUE
          /\ UNCHANGED discovered
     ELSE
          /\ discovered' = (discovered \/ (demanded /\ ~full))
          /\ UNCHANGED <<running, notified>>
  /\ UNCHANGED <<full, saturated, demanded, stopped, fault>>
Stop ==
  /\ running
  /\ running' = FALSE
  /\ stopped' = TRUE
  /\ notified' = TRUE
  /\ UNCHANGED <<full, saturated, demanded, discovered, fault>>
Fail ==
  /\ running
  /\ running' = FALSE
  /\ fault' = TRUE
  /\ notified' = (Bug # "skip-cleanup")
  /\ UNCHANGED <<full, saturated, demanded, discovered, stopped>>
Next == Demand \/ Saturate \/ Drain \/ Discover \/ Stop \/ Fail \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars /\ WF_vars(Drain) /\ WF_vars(Discover)

\* --------------------------------------------------------------- invariants --
TypeOK == \A value \in {full, saturated, running, demanded, discovered, stopped, fault, notified} : value \in BOOLEAN
ExitedCoordinatorNotifiesDrivers == ~running => notified

\* --------------------------------------------------------------- properties --
DiscoverySurvivesBackpressure == demanded ~> (discovered \/ stopped \/ fault)
=============================================================================
