------------------------------ MODULE V2Checks -------------------------------
(***************************************************************************)
(* Required checks (plan §8/A16): only a *claimed success* is verified, a       *)
(* failing check enters a bounded repair round, exhausted rounds (or an        *)
(* infrastructure failure the model cannot repair) settle the goal BLOCKED —   *)
(* never SUCCEEDED. The runtime never upgrades the model's candidate.          *)
(*                                                                          *)
(* The contract's *ingress* is modelled too (D-385): `require_checks` lets the  *)
(* user attach acceptance commands to a goal that has not started verifying,    *)
(* so `required` is a variable and a round records which set it actually ran    *)
(* (`roundChecks`). A check added after a claim would never be run by the       *)
(* registered round, which is why the rule is "before any candidate" and why    *)
(* the negative control sets `LateRequire = TRUE` to break exactly that.        *)
(*                                                                          *)
(* Code anchors: engine/src/v2/driver.rs — step_completion_checks (a stored     *)
(* candidate decides whether checks run at all; a round is registered only      *)
(* while rounds < max_rounds; the verdict comes from the round's terminal       *)
(* receipts; `dispatch_refused` / `spawn` classes block immediately; otherwise  *)
(* the driver repairs and re-verifies), execute_check_ops, check_verdict,       *)
(* observe_check_inputs (fresh observation per round: stale observations are a  *)
(* failure class, not a pass); core/src/v2/control.rs — require_checks (a       *)
(* user/project ingress; refused once a round is registered), register_check_runs, *)
(* validate_required_checks (user/project-defined contracts only),              *)
(* repair_completion, block_goal, complete_goal (closes with the *stored*       *)
(* candidate outcome).                                                          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS AllChecks,   \* every check a client could require, e.g. {"k1","k2"}
          Independent, \* D-393: do the goal's limits require a round another instance registered?
          SelfVerified, \* counterfactual (D-393): accept a round the producing instance registered itself
          MaxRounds,   \* repair-round budget, e.g. 2
          RewindRounds, \* counterfactual (D-219): opening a round resets the counter instead of advancing it
          LateRequire, \* counterfactual (D-385): a check may join after a claim, so no round covers it
          BriefWithoutChecks, \* counterfactual (D-418): the instance is told about checks the goal does not require
          \* D-462 counterfactuals, one per claim:
          BriefTwice,         \* the instance is briefed a second time
          RoundWithoutClaim,  \* a round is registered for a candidate that claims nothing
          UpgradeCandidate,   \* a failed candidate settles as SUCCEEDED
          UnverifiedAccept,   \* a success settles with no passing verdict at all
          UnboundedRounds,    \* a round opens past the repair budget
          BlockWithoutCause   \* the goal blocks with no exhausted budget and no stale check

ASSUME AllChecks # {} /\ MaxRounds > 0

Registrar == {"producer", "verifier"}
Producer == "producer"
Verifier == "verifier"
Outcomes == {"none", "success", "failed", "blocked"}
GoalStatus == {"ACTIVE", "SUCCEEDED", "BLOCKED", "FAILED"}
CheckResult == {"none", "pass", "fail", "stale"}
Verdict == {"none", "pass", "fail", "infra", "stale"}

VARIABLES
  candidate,   \* the stored completion candidate's outcome
  goalStatus,  \* the goal's status
  required,    \* the checks the goal's `limits.required_checks` names *now*
  round,       \* rounds registered so far
  openOps,     \* open check operations of the current round
  result,      \* check -> this round's result
  roundChecks, \* the checks the registered round was computed over
  registrar,   \* who registered the current round (D-393): the producing instance or a verifier
  briefings,   \* D-418: how many times the instance was told the goal's required checks
  lastVerdict, \* the verdict of the last completed round
  roundOpen,   \* is a round registered but not yet verdicted
  upgrades,    \* monitor: goals that succeeded without a passing verdict
  lateRound,   \* monitor: a round registered for a non-success candidate
  rewound,     \* monitor: the round counter ever went backwards
  lateJoin, \* monitor: a check joined the contract after a claim
  nonSuccessSuccess \* monitor: a failed/blocked candidate ended SUCCEEDED

vars == <<candidate, goalStatus, required, round, openOps, result, roundChecks, registrar, lastVerdict, roundOpen,
          briefings>>
monVars == <<vars, upgrades, lateRound, rewound, lateJoin, nonSuccessSuccess>>

\* ------------------------------------------------------------------- actions --
\* D-418: the runtime tells the instance, once, which checks the goal must satisfy. It is a *briefing*, not a
\* verdict: it states the criterion while the work is planned, where the gate that enforces it runs only at the
\* completion boundary (`NextCheck`/`ComputeVerdict` below). The text is the runtime's own composition of the
\* goal's stored contract, so a goal with nothing required is told nothing — the counterfactual that briefs
\* anyway must refute `BriefingIsForAGoalWithChecks`.
Brief ==
  /\ goalStatus = "ACTIVE"
  /\ (BriefWithoutChecks \/ required # {})
  /\ (BriefTwice \/ briefings = 0)
  /\ briefings' = IF BriefTwice THEN briefings + 1 ELSE 1
  /\ UNCHANGED <<candidate, goalStatus, required, round, openOps, result, roundChecks, registrar, lastVerdict,
                 roundOpen, upgrades, lateRound, rewound, lateJoin, nonSuccessSuccess>>

\* `require_checks` (D-385): the user's acceptance joins the goal. Only before the
\* model has claimed anything — a round decides from its own stored receipts, so a
\* check added later would never be run and the goal could still be accepted.
RequireCheck(c) ==
  /\ goalStatus = "ACTIVE"
  /\ c \in AllChecks
  /\ c \notin required
  /\ (candidate = "none" \/ LateRequire)
  /\ required' = required \cup {c}
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ lateJoin' = (lateJoin \/ (candidate # "none"))
  /\ UNCHANGED <<candidate, goalStatus, round, openOps, result, roundChecks, lastVerdict, roundOpen, registrar, briefings>>

\* The finish import stores the model's candidate; it never moves the goal.
StoreCandidate(outcome) ==
  /\ candidate = "none"
  /\ outcome \in {"success", "failed", "blocked"}
  /\ candidate' = outcome
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<goalStatus, required, round, openOps, result, roundChecks, lastVerdict, roundOpen, registrar, briefings>>

\* A candidate that does not claim success is settled as itself — no check runs
\* for it (the driver verifies only a claimed success, §8).
SettleWithoutChecks ==
  /\ candidate \in {"failed", "blocked"}
  /\ goalStatus = "ACTIVE"
  /\ goalStatus' = IF UpgradeCandidate THEN "SUCCEEDED" ELSE (IF candidate = "failed" THEN "FAILED" ELSE "BLOCKED")
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = IF UpgradeCandidate THEN TRUE ELSE nonSuccessSuccess
  /\ UNCHANGED <<candidate, required, round, openOps, result, roundChecks, lastVerdict, roundOpen, registrar, briefings>>

\* A claimed success with an empty contract settles on the candidate alone
\* (`step_completion`: no checks configured ⇒ `complete_goal`).
SettleSuccessWithoutChecks ==
  /\ candidate = "success"
  /\ required = {}
  /\ goalStatus = "ACTIVE"
  /\ goalStatus' = "SUCCEEDED"
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, required, round, openOps, result, roundChecks, lastVerdict, roundOpen, registrar, briefings>>

\* register_check_runs: a fresh round opens for the claimed success over the
\* contract *as it stands now*, with newly observed inputs for every check
\* (rounds < budget).
RegisterRound ==
  /\ (RoundWithoutClaim \/ candidate = "success")
  /\ goalStatus = "ACTIVE"
  /\ required # {}
  /\ ~roundOpen
  /\ (UnboundedRounds \/ round < MaxRounds)
  /\ round' = IF RewindRounds /\ round > 0 THEN 0 ELSE round + 1
  /\ roundOpen' = TRUE
  /\ openOps' = 1
  /\ roundChecks' = required
  /\ registrar' = Producer
  /\ result' = [ k \in AllChecks |-> "none" ]
  /\ upgrades' = upgrades
  /\ rewound' = IF RewindRounds /\ round > 0 THEN TRUE ELSE rewound
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ lateRound' = IF candidate = "success" THEN lateRound ELSE TRUE
  /\ lateJoin' = lateJoin
  /\ UNCHANGED <<candidate, goalStatus, required, lastVerdict, briefings>>

\* D-393: another instance runs the same contract and registers the round. This is what the
\* `verify_goal` tool does mid-turn (`register_verification`), and the only round an independent goal
\* may settle on.
RegisterVerifierRound ==
  /\ candidate = "success"
  /\ goalStatus = "ACTIVE"
  /\ required # {}
  /\ roundOpen
  /\ registrar = Producer
  /\ registrar' = Verifier
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, goalStatus, required, round, openOps, result, roundChecks, lastVerdict, roundOpen, briefings>>

\* ... this version runs one check operation per step (execute_check_ops); each
\* terminal receipt lands as a result — and only for a check the round runs.
LandResult(k, r) ==
  /\ roundOpen
  /\ openOps > 0
  /\ k \in roundChecks
  /\ result[k] = "none"
  /\ result' = [result EXCEPT ![k] = r]
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, goalStatus, required, round, openOps, roundChecks, lastVerdict, roundOpen, registrar, briefings>>

\* D-462 counterfactual: a claimed success settles with no passing verdict.
AcceptUnverified ==
  /\ UnverifiedAccept
  /\ goalStatus = "ACTIVE"
  /\ goalStatus' = "SUCCEEDED"
  /\ upgrades' = TRUE
  /\ UNCHANGED <<candidate, required, round, openOps, result, roundChecks, lastVerdict, roundOpen, registrar,
                 briefings, lateRound, rewound, lateJoin, nonSuccessSuccess>>

NextCheck ==
  /\ roundOpen
  /\ openOps > 0
  /\ openOps' = openOps - 1
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, goalStatus, required, round, result, roundChecks, lastVerdict, roundOpen, registrar, briefings>>

\* The verdict of a completed round: all pass ⇒ pass; an unavailable
\* verification path (dispatch refused / runner never started) or a stale
\* observation is its own class (never a pass).
ComputeVerdict ==
  /\ roundOpen
  /\ openOps = 0
  /\ \A k \in roundChecks : result[k] # "none"
  /\ LET verdict == IF \A k \in roundChecks : result[k] = "pass" THEN "pass"
       ELSE IF \E k \in roundChecks : result[k] = "stale" THEN "stale"
       ELSE "fail" IN
     /\ lastVerdict' = verdict
     /\ roundOpen' = FALSE
     /\ upgrades' = upgrades
     /\ lateRound' = lateRound
     /\ rewound' = rewound
     /\ lateJoin' = lateJoin
     /\ nonSuccessSuccess' = nonSuccessSuccess
     /\ UNCHANGED <<candidate, goalStatus, required, round, openOps, result, roundChecks, registrar, briefings>>

\* Infrastructure failures are not model-repairable: they block at once.
BlockForInfra ==
  /\ roundOpen
  /\ openOps = 0
  /\ goalStatus = "ACTIVE"
  \* a stale observation means the verification path itself is unavailable —
  \* like a refused dispatch, the model cannot repair it
  /\ \E k \in roundChecks : (BlockWithoutCause \/ result[k] = "stale")
  /\ goalStatus' = "BLOCKED"
  /\ roundOpen' = FALSE
  /\ lastVerdict' = "stale"
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, required, round, openOps, result, roundChecks, registrar, briefings>>

\* A passing verdict closes the goal with the *stored* candidate — over the round's
\* own checks, which is the code's shape and the reason `LateRequire` refutes
\* `EveryRequiredCheckWasVerified`.
Accept ==
  /\ roundOpen
  /\ openOps = 0
  /\ \A k \in roundChecks : result[k] = "pass"
  \* D-393: an independent goal settles only on a round a *different* instance registered.
  /\ (~Independent \/ registrar = Verifier \/ SelfVerified)
  /\ goalStatus = "ACTIVE"
  /\ goalStatus' = "SUCCEEDED"
  /\ lastVerdict' = "pass"
  /\ roundOpen' = FALSE
  /\ upgrades' = IF candidate = "success" THEN upgrades ELSE TRUE
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = IF candidate = "success" THEN nonSuccessSuccess ELSE TRUE
  /\ UNCHANGED <<candidate, required, round, openOps, result, roundChecks, registrar, briefings>>

\* A failing round below the budget buys another repair turn: the goal stays
\* ACTIVE, the round is cleared so the driver can register the next one.
Repair ==
  /\ roundOpen
  /\ openOps = 0
  /\ \E k \in roundChecks : result[k] = "fail"
  /\ \A k \in roundChecks : result[k] # "stale"
  /\ goalStatus = "ACTIVE"
  /\ round < MaxRounds
  /\ roundOpen' = FALSE
  /\ lastVerdict' = "fail"
  /\ upgrades' = upgrades
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ lateRound' = lateRound
  /\ UNCHANGED <<candidate, goalStatus, required, round, openOps, result, roundChecks, registrar, briefings>>

\* Exhausted budget: the goal is settled BLOCKED, never upgraded
\* (complete_goal is never reached with a failing verdict).
BlockWhenExhausted ==
  /\ \/ (roundOpen /\ openOps = 0 /\ (\E k \in roundChecks : result[k] = "fail")
                       /\ (\A j \in roundChecks : result[j] # "stale") /\ round >= MaxRounds)
     \/ (~roundOpen /\ round >= MaxRounds /\ goalStatus = "ACTIVE" /\ candidate = "success")
  /\ goalStatus' = "BLOCKED"
  /\ roundOpen' = FALSE
  /\ upgrades' = upgrades
  /\ lateRound' = lateRound
  /\ rewound' = rewound
  /\ lateJoin' = lateJoin
  /\ nonSuccessSuccess' = nonSuccessSuccess
  /\ UNCHANGED <<candidate, required, round, openOps, result, roundChecks, lastVerdict, registrar, briefings>>

Stutter == UNCHANGED monVars

Next ==
  \/ Brief
  \/ \E c \in AllChecks : RequireCheck(c)
  \/ \E o \in {"success", "failed", "blocked"} : StoreCandidate(o)
  \/ SettleWithoutChecks
  \/ SettleSuccessWithoutChecks
  \/ RegisterRound
  \/ RegisterVerifierRound
  \/ \E k \in AllChecks : \E r \in {"pass", "fail", "stale"} : LandResult(k, r)
  \/ NextCheck
  \/ ComputeVerdict
  \/ BlockForInfra
  \/ Accept
  \/ AcceptUnverified
  \/ Repair
  \/ BlockWhenExhausted
  \/ Stutter

Init ==
  /\ candidate = "none"
  /\ goalStatus = "ACTIVE"
  /\ required = {}
  /\ round = 0
  /\ openOps = 0
  /\ result = [ k \in AllChecks |-> "none" ]
  /\ roundChecks = {}
  /\ registrar = Producer
  /\ lastVerdict = "none"
  /\ roundOpen = FALSE
  /\ briefings = 0
  /\ upgrades = FALSE
  /\ lateRound = FALSE
  /\ rewound = FALSE
  /\ lateJoin = FALSE
  /\ nonSuccessSuccess = FALSE

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ candidate \in Outcomes
  /\ goalStatus \in GoalStatus
  /\ required \subseteq AllChecks
  /\ roundChecks \subseteq AllChecks
  /\ registrar \in Registrar
  /\ round \in 0..MaxRounds
  /\ openOps \in 0..1
  /\ \A k \in AllChecks : result[k] \in CheckResult
  /\ lastVerdict \in Verdict
  /\ roundOpen \in BOOLEAN

\* A16: a goal only reaches SUCCEEDED when every *required* check really passed,
\* and it passed in the round the goal was closed on. This is stated over the
\* *observed results* (not over the recorded verdict): writing "pass" into a
\* verdict variable is exactly the kind of self-fulfilling claim this invariant
\* must not accept. `LateRequire` is the control that breaks the second half.
\* D-418: the instance is told once, and only about a contract that exists. Two claims, because the first is a
\* state fact and the second is about the step that writes the note.
BriefingIsOnce ==
  briefings <= 1

\* One briefing per goal, and only while the goal is live and really requires something.
BriefingIsForAGoalWithChecks ==
  [][ (briefings' > briefings) => (required # {} /\ goalStatus = "ACTIVE") ]_vars

EveryRequiredCheckWasVerified ==
  goalStatus = "SUCCEEDED" => (\A k \in required : k \in roundChecks /\ result[k] = "pass")

\* D-393: a goal that requires independent verification never settles on the producing instance's own round.
\* The rule is about a settlement *on a round*: a goal with no required checks has nothing an independent pass
\* could run, so it settles on its candidate alone (`round = 0`) and the flag never applies to it.
IndependenceIsNotSelfVerified ==
  (goalStatus = "SUCCEEDED" /\ round > 0) => (~Independent \/ registrar = Verifier)

\* §8: the runtime never upgrades the model's candidate — a candidate that
\* admits undelivered work cannot end SUCCEEDED (monitored too)
NoUpgradeOfTheCandidate ==
  /\ nonSuccessSuccess = FALSE
  /\ candidate \in {"failed", "blocked"} => goalStatus # "SUCCEEDED"

\* checks are only run for a claimed success (monitored)
ChecksOnlyVerifyAClaimedSuccess == lateRound = FALSE

\* the contract only ever grows, and every growth happens before a claim
ChecksOnlyJoinBeforeAClaim == lateJoin = FALSE

\* the round counter never goes backwards, and it never exceeds the budget
RoundsAreMonotone == rewound = FALSE
RoundsAreBounded == round <= MaxRounds

\* a claimed success that ends BLOCKED means either the repair budget ran out or
\* the verification path itself was unavailable (stale observation / refused
\* dispatch) — never a silent close
BlockedAfterTheBudgetOrStale ==
  candidate = "success" /\ goalStatus = "BLOCKED" =>
    round >= MaxRounds \/ (\E k \in roundChecks : result[k] = "stale")

\* no goal ever succeeded without a passing verdict (monitored)
NoUnverifiedSuccess == upgrades = FALSE

=============================================================================
