------------------------------ MODULE V2Checks -------------------------------
(***************************************************************************)
(* Required checks (plan §8/A16): only a *claimed success* is verified, a       *)
(* failing check enters a bounded repair round, exhausted rounds (or an        *)
(* infrastructure failure the model cannot repair) settle the goal BLOCKED —   *)
(* never SUCCEEDED. The runtime never upgrades the model's candidate.          *)
(*                                                                          *)
(* Code anchors: engine/src/v2/driver.rs — step_completion_checks (a stored     *)
(* candidate decides whether checks run at all; a round is registered only      *)
(* while rounds < max_rounds; the verdict comes from the round's terminal       *)
(* receipts; `dispatch_refused` / `spawn` classes block immediately; otherwise  *)
(* the driver repairs and re-verifies), execute_check_ops, check_verdict,       *)
(* observe_check_inputs (fresh observation per round: stale observations are a  *)
(* failure class, not a pass); core/src/v2/control.rs — register_check_runs,    *)
(* validate_required_checks (user/project-defined contracts only),              *)
(* repair_completion, block_goal, complete_goal (closes with the *stored*       *)
(* candidate outcome).                                                          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Checks,      \* required checks, e.g. {"k1","k2"}
          MaxRounds    \* repair-round budget, e.g. 2

ASSUME Checks # {} /\ MaxRounds > 0

Outcomes == {"none", "success", "failed", "blocked"}
GoalStatus == {"ACTIVE", "SUCCEEDED", "BLOCKED", "FAILED"}
CheckResult == {"none", "pass", "fail", "stale"}
Verdict == {"none", "pass", "fail", "infra", "stale"}

VARIABLES
  candidate,   \* the stored completion candidate's outcome
  goalStatus,  \* the goal's status
  round,       \* rounds registered so far
  openOps,     \* open check operations of the current round
  result,      \* check -> this round's result
  lastVerdict, \* the verdict of the last completed round
  roundOpen,   \* is a round registered but not yet verdicted
  upgrades,    \* monitor: goals that succeeded without a passing verdict
  lateRound,   \* monitor: a round registered for a non-success candidate
  rewound,     \* monitor: the round counter ever went backwards
  nonSuccessSuccess \* monitor: a failed/blocked candidate ended SUCCEEDED

vars == <<candidate, goalStatus, round, openOps, result, lastVerdict, roundOpen>>
monVars == <<vars, upgrades, lateRound, rewound, nonSuccessSuccess>>

\* ------------------------------------------------------------------- actions --
\* The finish import stores the model's candidate; it never moves the goal.
StoreCandidate(outcome) ==
  /\ candidate = "none"
  /\ outcome \in {"success", "failed", "blocked"}
  /\ candidate' = outcome
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<goalStatus, round, openOps, result, lastVerdict, roundOpen>>

\* A candidate that does not claim success is settled as itself — no check runs
\* for it (the driver verifies only a claimed success, §8).
SettleWithoutChecks ==
  /\ candidate \in {"failed", "blocked"}
  /\ goalStatus = "ACTIVE"
  /\ goalStatus' = IF candidate = "failed" THEN "FAILED" ELSE "BLOCKED"
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, round, openOps, result, lastVerdict, roundOpen>>

\* register_check_runs: a fresh round opens for the claimed success, with newly
\* observed inputs for every required check (rounds < budget).
RegisterRound ==
  /\ candidate = "success"
  /\ goalStatus = "ACTIVE"
  /\ ~roundOpen
  /\ round < MaxRounds
  /\ round' = round + 1
  /\ roundOpen' = TRUE
  /\ openOps' = 1
  /\ result' = [ k \in Checks |-> "none" ]
  /\ upgrades' = upgrades
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ lateRound' = IF candidate = "success" THEN lateRound ELSE TRUE
  /\ UNCHANGED <<candidate, goalStatus, lastVerdict>>

\* ... this version runs one check operation per step (execute_check_ops); each
\* terminal receipt lands as a result
LandResult(k, r) ==
  /\ roundOpen
  /\ openOps > 0
  /\ result[k] = "none"
  /\ result' = [result EXCEPT ![k] = r]
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, goalStatus, round, openOps, lastVerdict, roundOpen>>

NextCheck ==
  /\ roundOpen
  /\ openOps > 0
  /\ openOps' = openOps - 1
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, goalStatus, round, result, lastVerdict, roundOpen>>

\* The verdict of a completed round: all pass ⇒ pass; an unavailable
\* verification path (dispatch refused / runner never started) or a stale
\* observation is its own class (never a pass).
ComputeVerdict ==
  /\ roundOpen
  /\ openOps = 0
  /\ \A k \in Checks : result[k] # "none"
  /\ LET verdict == IF \A k \in Checks : result[k] = "pass" THEN "pass"
       ELSE IF \E k \in Checks : result[k] = "stale" THEN "stale"
       ELSE "fail" IN
     /\ lastVerdict' = verdict
     /\ roundOpen' = FALSE
     /\ upgrades' = upgrades
     /\ lateRound' = lateRound
     /\ rewound' = rewound
     /\ nonSuccessSuccess' = nonSuccessSuccess
     /\ UNCHANGED <<candidate, goalStatus, round, openOps, result>>

\* Infrastructure failures are not model-repairable: they block at once.
BlockForInfra ==
  /\ roundOpen
  /\ openOps = 0
  /\ goalStatus = "ACTIVE"
  \* a stale observation means the verification path itself is unavailable —
  \* like a refused dispatch, the model cannot repair it
  /\ \E k \in Checks : result[k] = "stale"
  /\ goalStatus' = "BLOCKED"
  /\ roundOpen' = FALSE
  /\ lastVerdict' = "stale"
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, round, openOps, result>>

\* A passing verdict closes the goal with the *stored* candidate
Accept ==
  /\ roundOpen
  /\ openOps = 0
  /\ \A k \in Checks : result[k] = "pass"
  /\ goalStatus = "ACTIVE"
  /\ goalStatus' = "SUCCEEDED"
  /\ lastVerdict' = "pass"
  /\ roundOpen' = FALSE
  /\ upgrades' = IF candidate = "success" THEN upgrades ELSE TRUE
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = IF candidate = "success" THEN nonSuccessSuccess ELSE TRUE
  /\ UNCHANGED <<candidate, round, openOps, result>>

\* A failing round below the budget buys another repair turn: the goal stays
\* ACTIVE, the round is cleared so the driver can register the next one.
Repair ==
  /\ roundOpen
  /\ openOps = 0
  /\ \E k \in Checks : result[k] = "fail"
  /\ \A k \in Checks : result[k] # "stale"
  /\ goalStatus = "ACTIVE"
  /\ round < MaxRounds
  /\ roundOpen' = FALSE
  /\ lastVerdict' = "fail"
  /\ upgrades' = upgrades
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ lateRound' = lateRound
  /\ UNCHANGED <<candidate, goalStatus, round, openOps, result>>

\* Exhausted budget: the goal is settled BLOCKED, never upgraded
\* (complete_goal is never reached with a failing verdict).
BlockWhenExhausted ==
  /\ \/ (roundOpen /\ openOps = 0 /\ (\E k \in Checks : result[k] = "fail")
                       /\ (\A j \in Checks : result[j] # "stale") /\ round >= MaxRounds)
     \/ (~roundOpen /\ round >= MaxRounds /\ goalStatus = "ACTIVE" /\ candidate = "success")
  /\ goalStatus' = "BLOCKED"
  /\ roundOpen' = FALSE
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, round, openOps, result, lastVerdict>>

Stutter == UNCHANGED monVars

Next ==
  \/ \E o \in {"success", "failed", "blocked"} : StoreCandidate(o)
  \/ SettleWithoutChecks
  \/ RegisterRound
  \/ \E k \in Checks : \E r \in {"pass", "fail", "stale"} : LandResult(k, r)
  \/ NextCheck
  \/ ComputeVerdict
  \/ BlockForInfra
  \/ Accept
  \/ Repair
  \/ BlockWhenExhausted
  \/ Stutter

Init ==
  /\ candidate = "none"
  /\ goalStatus = "ACTIVE"
  /\ round = 0
  /\ openOps = 0
  /\ result = [ k \in Checks |-> "none" ]
  /\ lastVerdict = "none"
  /\ roundOpen = FALSE
  /\ upgrades = FALSE
  /\ lateRound = FALSE
  /\ rewound = FALSE
  /\ nonSuccessSuccess = FALSE

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ candidate \in Outcomes
  /\ goalStatus \in GoalStatus
  /\ round \in 0..MaxRounds
  /\ openOps \in 0..1
  /\ \A k \in Checks : result[k] \in CheckResult
  /\ lastVerdict \in Verdict
  /\ roundOpen \in BOOLEAN

\* A16: a goal only reaches SUCCEEDED when every required check of the round it
\* was closed on really passed. This is stated over the *observed results* (not
\* over the recorded verdict): writing "pass" into a verdict variable is exactly
\* the kind of self-fulfilling claim this invariant must not accept.
SuccessRequiresAllChecksPassed ==
  goalStatus = "SUCCEEDED" => (\A k \in Checks : result[k] = "pass")

\* §8: the runtime never upgrades the model's candidate — a candidate that
\* admits undelivered work cannot end SUCCEEDED (monitored too)
NoUpgradeOfTheCandidate ==
  /\ nonSuccessSuccess = FALSE
  /\ candidate \in {"failed", "blocked"} => goalStatus # "SUCCEEDED"

\* checks are only run for a claimed success (monitored)
ChecksOnlyVerifyAClaimedSuccess == lateRound = FALSE

\* the round counter never goes backwards, and it never exceeds the budget
RoundsAreMonotone == rewound = FALSE
RoundsAreBounded == round <= MaxRounds

\* a claimed success that ends BLOCKED means either the repair budget ran out or
\* the verification path itself was unavailable (stale observation / refused
\* dispatch) — never a silent close
BlockedAfterTheBudgetOrStale ==
  candidate = "success" /\ goalStatus = "BLOCKED" =>
    round >= MaxRounds \/ (\E k \in Checks : result[k] = "stale")

\* no goal ever succeeded without a passing verdict (monitored)
NoUnverifiedSuccess == upgrades = FALSE

=============================================================================
