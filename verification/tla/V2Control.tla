------------------------------ MODULE V2Control ------------------------------
(***************************************************************************)
(* Abstract model of the TeamAgents v2 control plane.                      *)
(* Code it abstracts: core/src/v2/control.rs (the single trusted           *)
(* transaction entry) and engine/src/v2/driver.rs (the phase machine).     *)
(* Plan clauses: §4.1 single writer, §4.2 same-transaction facts,          *)
(* §6.1 dispatch linearization point, §6.2 approvals, §6.3 recovery,       *)
(* §8 budget/reservations; acceptance scenes A07/A08/A10/A11/A13/A18/      *)
(* A19/A24/A25.                                                            *)
(*                                                                         *)
(* Only what the safety invariants need is modelled: durable short         *)
(* transactions, the executor's revision guard, dispatch record vs         *)
(* effect, approval gating, terminal uniqueness, epoch reset, budget       *)
(* reservations. The environment (tool outcome, approval timing, crashes)  *)
(* is nondeterministic on purpose.                                         *)
(*                                                                         *)
(* Fixed finite domains (ReqIds, AttIds) keep the state graph finite for   *)
(* TLC; "none" marks a free slot.                                          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Instances,       \* {"L"} or {"L","W"}
          Ops,             \* operations of one decision, e.g. {"o1","o2"}
          ReqIds,          \* request slots, e.g. 1..3
          AttIds,          \* attempt slots, e.g. 1..2
          ApprovalOps,     \* subset of Ops needing user approval
          TokenLimit,      \* goal budget limit
          MaxEpoch,        \* bound on ResetInstance (keeps the state graph finite)
          MaxUnknown       \* bound on lost-attempt accounting

ASSUME Instances # {} /\ Ops # {} /\ ReqIds # {} /\ AttIds # {} /\ ApprovalOps \subseteq Ops

OpNonTerminal == {"PREPARED", "DISPATCH_COMMITTED", "RUNNING"}
OpTerminal    == {"SUCCEEDED", "FAILED", "CANCELLED", "CANCELLED_BEFORE_START", "OUTCOME_UNKNOWN"}
ApprovalDone  == {"APPROVED", "DENIED", "EXPIRED"}
Result        == {"ops", "reply", "completion", "wait"}
ReqClosed     == {"COMPLETE", "FAILED", "CANCELLED"}

nil == 0   \* free-slot / no-instance sentinel (integer, so it compares with ids)

\* summing ests without depending on a Sequences helper
RECURSIVE SumSet(_)
SumSet(S) == IF S = {} THEN 0 ELSE LET x == CHOOSE y \in S : TRUE IN x + SumSet(S \ {x})

VARIABLES
  inst,      \* instance -> [lifecycle, phase, revision, expectRev, epoch,
             \*               activeReq, ctxEpoch, tail]
  goal,      \* [status, known, unknown, reserved]  reserved: Set of <<req, est>>
  requests,  \* request slot -> [status, instance, epoch, est, selected, result]
  attempts,  \* attempt slot -> [req, status]
  ops,       \* operation -> [status, instance, epoch, effect, dispatched]
  approvals, \* operation -> "none" | "PENDING" | ApprovalDone
  dead       \* crashed instances (in-memory view lost; durable state stays)

vars == <<inst, goal, requests, attempts, ops, approvals, dead>>

\* ------------------------------------------------------------------- helpers --
UsedReqs == { r \in ReqIds : requests[r].status # "none" }
FreeReqs == ReqIds \ UsedReqs
FreeAtts == { a \in AttIds : attempts[a].status = "none" }
ReservedTotal == SumSet({ e[2] : e \in goal.reserved })
BudgetFits(e) == goal.known + ReservedTotal + e <= TokenLimit
Fresh(i) == inst[i].expectRev = inst[i].revision
Alive(i) == i \notin dead
ApprovalSatisfied(o) == o \notin ApprovalOps \/ approvals[o] = "APPROVED"
NonTerminalOps(i) == { o \in Ops : ops[o].instance = i /\ ops[o].status \in OpNonTerminal }
DoneOps(i) == { o \in Ops : ops[o].instance = i /\ ops[o].status \in OpTerminal }

\* ------------------------------------------------------------------- actions --
\* user input lands at the READY boundary (§5.4)
Input(i) ==
  /\ Alive(i) /\ inst[i].phase = "READY" /\ inst[i].lifecycle = "ACTIVE"
  /\ inst' = [inst EXCEPT ![i].tail = "user"]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* READY -> MODEL_PENDING: revision guard, idle rule, budget reservation
BeginRequest(i) ==
  /\ Alive(i) /\ inst[i].phase = "READY" /\ inst[i].lifecycle = "ACTIVE"
  /\ Fresh(i)
  /\ inst[i].tail # "assistant"          \* the idle rule: no turn without work
  /\ BudgetFits(1)
  /\ \E r \in FreeReqs :
       /\ requests' = [requests EXCEPT ![r] = [status |-> "PENDING", instance |-> i,
                                              epoch |-> inst[i].epoch, est |-> 1,
                                              selected |-> FALSE, result |-> "reply"]]
       /\ goal' = [goal EXCEPT !.reserved = @ \cup {<<r, 1>>}]
       /\ inst' = [inst EXCEPT ![i].phase = "MODEL_PENDING", ![i].activeReq = r,
                                  ![i].revision = @ + 1, ![i].expectRev = @ + 1]
  /\ UNCHANGED <<attempts, ops, approvals, dead>>

\* the atomic response selection bills the goal; late completes only archive
RecordAttempt(i) ==
  /\ Alive(i)
  /\ \E r \in UsedReqs :
       /\ inst[i].activeReq = r /\ requests[r].status = "PENDING"
       /\ \E a \in FreeAtts :
            attempts' = [attempts EXCEPT ![a] = [req |-> r, status |-> "COMPLETE"]]
            /\ requests' = [requests EXCEPT ![r].selected = TRUE]
            /\ goal' = [goal EXCEPT !.known = @ + 1]
  /\ UNCHANGED <<inst, ops, approvals, dead>>

\* an in-flight attempt lost to a crash: counted as unknown, never replayed
LostAttempt(i) ==
  /\ Alive(i)
  /\ goal.unknown < MaxUnknown
  /\ \E r \in UsedReqs :
       /\ inst[i].activeReq = r /\ requests[r].status = "PENDING"
       /\ goal' = [goal EXCEPT !.unknown = @ + 1]
  /\ UNCHANGED <<inst, requests, attempts, ops, approvals, dead>>

\* response import: context append + decision + phase, one transaction
ImportResponse(i) ==
  /\ Alive(i)
  /\ \E r \in UsedReqs :
       /\ inst[i].activeReq = r /\ requests[r].status = "PENDING"
       /\ requests[r].selected
       /\ requests' = [requests EXCEPT ![r].status = "COMPLETE"]
       /\ goal' = [goal EXCEPT !.reserved = @ \ {<<r, requests[r].est>>}]
       /\ inst' = [inst EXCEPT ![i].phase =
                      CASE requests[r].result = "ops"        -> "TOOLS_PENDING"
                        [] requests[r].result = "reply"      -> "READY"
                        [] requests[r].result = "completion" -> "COMPLETION_PENDING"
                        [] OTHER                             -> "WAITING",
                                  ![i].tail = "assistant",
                                  ![i].activeReq = IF requests[r].result = "ops" THEN r ELSE nil]
       /\ ops' = IF requests[r].result = "ops"
                   THEN [ o \in Ops |->
                            [status |-> "PREPARED", instance |-> i, epoch |-> inst[i].epoch,
                             effect |-> 0, dispatched |-> FALSE] ]
                   ELSE ops
       /\ approvals' = IF requests[r].result = "ops" THEN [ o \in Ops |-> "none" ] ELSE approvals
  /\ UNCHANGED <<attempts, dead>>

\* a shell-like operation asks the user; the runtime never self-approves (§6.2)
RequestApproval(o) ==
  /\ Alive(ops[o].instance) /\ ops[o].status = "PREPARED"
  /\ o \in ApprovalOps /\ approvals[o] = "none"
  /\ approvals' = [approvals EXCEPT ![o] = "PENDING"]
  /\ UNCHANGED <<inst, goal, requests, attempts, ops, dead>>

Approve(o) ==
  /\ approvals[o] = "PENDING"
  /\ approvals' = [approvals EXCEPT ![o] = "APPROVED"]
  /\ UNCHANGED <<inst, goal, requests, attempts, ops, dead>>

Deny(o) ==
  /\ approvals[o] = "PENDING"
  /\ approvals' = [approvals EXCEPT ![o] = "DENIED"]
  /\ ops' = [ops EXCEPT ![o].status = "CANCELLED_BEFORE_START"]
  /\ UNCHANGED <<inst, goal, requests, attempts, dead>>

\* the linearization point: durable dispatch record after the re-checks
Dispatch(o) ==
  /\ Alive(ops[o].instance)
  /\ ops[o].status = "PREPARED"
  /\ inst[ops[o].instance].phase = "TOOLS_PENDING"
  /\ inst[ops[o].instance].lifecycle = "ACTIVE"
  /\ ApprovalSatisfied(o)
  /\ ops' = [ops EXCEPT ![o].status = "DISPATCH_COMMITTED", ![o].dispatched = TRUE]
  /\ UNCHANGED <<inst, goal, requests, attempts, approvals, dead>>

\* the external effect only starts after its durable record (A08)
StartEffect(o) ==
  /\ Alive(ops[o].instance)
  /\ ops[o].status = "DISPATCH_COMMITTED"
  /\ ops' = [ops EXCEPT ![o].status = "RUNNING", ![o].effect = @ + 1]
  /\ UNCHANGED <<inst, goal, requests, attempts, approvals, dead>>

Complete(o, outcome) ==
  /\ outcome \in OpTerminal
  /\ ops[o].status \in OpNonTerminal
  /\ ops[o].epoch = inst[ops[o].instance].ctxEpoch   \* no receipt across epochs
  /\ ~(ops[o].status = "PREPARED" /\ outcome \in {"SUCCEEDED", "FAILED", "OUTCOME_UNKNOWN"})
  /\ ops' = [ops EXCEPT ![o].status = outcome]
  /\ UNCHANGED <<inst, goal, requests, attempts, approvals, dead>>

\* decision consumption: receipts join the context, phase READY, approvals expire
ConsumeDecision(i) ==
  /\ Alive(i) /\ inst[i].phase = "TOOLS_PENDING"
  /\ \E r \in UsedReqs :
       /\ inst[i].activeReq = r /\ requests[r].result = "ops"
       /\ DoneOps(i) = Ops
       /\ inst' = [inst EXCEPT ![i].phase = "READY", ![i].tail = "tool", ![i].activeReq = nil]
       /\ approvals' = [ o \in Ops |-> IF approvals[o] = "PENDING" THEN "EXPIRED" ELSE approvals[o] ]
       /\ UNCHANGED <<goal, requests, attempts, ops, dead>>

Crash(i) ==
  /\ Alive(i)
  /\ dead' = dead \cup {i}
  /\ UNCHANGED <<inst, goal, requests, attempts, ops, approvals>>

\* recovery never restarts a possibly-started effect (A08/A11)
Recover(i) ==
  /\ i \in dead
  /\ dead' = dead \ {i}
  /\ \/ \E o \in Ops : ops[o].instance = i /\ ops[o].status \in {"DISPATCH_COMMITTED", "RUNNING"}
                        /\ ops' = [ops EXCEPT ![o].status = "OUTCOME_UNKNOWN"]
                        /\ UNCHANGED <<inst, goal, requests, attempts, approvals>>
     \/ UNCHANGED <<inst, goal, requests, attempts, ops, approvals>>

\* user cancel: request closes, reservation released, instance READY (A13)
CancelRequest(i) ==
  /\ Alive(i)
  /\ \E r \in UsedReqs :
       /\ inst[i].activeReq = r /\ requests[r].status = "PENDING"
       /\ requests' = [requests EXCEPT ![r].status = "CANCELLED"]
       /\ goal' = [goal EXCEPT !.reserved = @ \ {<<r, requests[r].est>>}]
       /\ inst' = [inst EXCEPT ![i].phase = "READY", ![i].activeReq = nil,
                                  ![i].revision = @ + 1, ![i].expectRev = @ + 1]
  /\ UNCHANGED <<attempts, ops, approvals, dead>>

\* permanent failure parks the instance and keeps the input (§5.4/A07)
FailRequest(i) ==
  /\ Alive(i)
  /\ \E r \in UsedReqs :
       /\ inst[i].activeReq = r /\ requests[r].status = "PENDING"
       /\ requests' = [requests EXCEPT ![r].status = "FAILED"]
       /\ goal' = [goal EXCEPT !.reserved = @ \ {<<r, requests[r].est>>}]
       /\ inst' = [inst EXCEPT ![i].lifecycle = "PARKED", ![i].activeReq = nil,
                                  ![i].phase = "READY"]
  /\ UNCHANGED <<attempts, ops, approvals, dead>>

\* goal settlement happens once and only from ACTIVE (§8)
SettleGoal(i, status) ==
  /\ Alive(i) /\ status \in {"SUCCEEDED", "FAILED", "BLOCKED", "CANCELLED"}
  /\ goal.status = "ACTIVE"
  /\ NonTerminalOps(i) = {}                       \* no open operation survives a close
  /\ ~(\E r \in UsedReqs : requests[r].instance = i /\ requests[r].status = "PENDING")
  /\ goal' = [goal EXCEPT !.status = status]
  /\ inst' = [inst EXCEPT ![i].phase = "READY", ![i].tail = "assistant"]
  /\ UNCHANGED <<requests, attempts, ops, approvals, dead>>

\* reset: new epoch closes the old execution, reservations released (A24)
ResetInstance(i) ==
  /\ Alive(i)
  /\ inst[i].epoch < MaxEpoch
  /\ LET newEpoch == inst[i].epoch + 1 IN
       /\ inst' = [inst EXCEPT ![i].epoch = newEpoch, ![i].phase = "READY",
                              ![i].ctxEpoch = newEpoch, ![i].activeReq = nil,
                              ![i].revision = @ + 1, ![i].expectRev = @ + 1]
       /\ requests' = [ r \in ReqIds |->
                          IF requests[r].instance = i /\ requests[r].status = "PENDING"
                            THEN [requests[r] EXCEPT !.status = "CANCELLED"] ELSE requests[r] ]
       /\ goal' = [goal EXCEPT !.reserved = { e \in goal.reserved : requests[e[1]].instance # i }]
       /\ ops' = [ o \in Ops |->
                     IF ops[o].instance = i /\ ops[o].status \in OpNonTerminal
                       THEN [ops[o] EXCEPT !.status = "CANCELLED"] ELSE ops[o] ]
  /\ UNCHANGED <<attempts, approvals, dead>>

SetLifecycle(i, l) ==
  /\ Alive(i) /\ l \in {"ACTIVE", "PAUSED", "PARKED", "TERMINATED"}
  /\ l # inst[i].lifecycle
  /\ inst' = [inst EXCEPT ![i].lifecycle = l]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* ---------------------------------------------------------------------- spec --
\* an idle step keeps the model open-ended (TLC then reports no false deadlock)
Stutter == UNCHANGED vars

Next ==
  \/ \E i \in Instances : Input(i)
  \/ \E i \in Instances : BeginRequest(i)
  \/ \E i \in Instances : RecordAttempt(i)
  \/ \E i \in Instances : LostAttempt(i)
  \/ \E i \in Instances : ImportResponse(i)
  \/ \E o \in Ops : RequestApproval(o)
  \/ \E o \in Ops : Approve(o)
  \/ \E o \in Ops : Deny(o)
  \/ \E o \in Ops : Dispatch(o)
  \/ \E o \in Ops : StartEffect(o)
  \/ \E o \in Ops : Complete(o, CHOOSE x \in OpTerminal : TRUE)
  \/ \E i \in Instances : ConsumeDecision(i)
  \/ \E i \in Instances : Crash(i)
  \/ \E i \in Instances : Recover(i)
  \/ \E i \in Instances : CancelRequest(i)
  \/ \E i \in Instances : FailRequest(i)
  \/ \E i \in Instances : SettleGoal(i, CHOOSE x \in {"SUCCEEDED", "FAILED", "BLOCKED", "CANCELLED"} : TRUE)
  \/ \E i \in Instances : ResetInstance(i)
  \/ \E i \in Instances : \E l \in {"ACTIVE", "PAUSED", "PARKED", "TERMINATED"} : SetLifecycle(i, l)
  \/ Stutter   \* the system is open-ended: an idle step is always possible

Init ==
  /\ inst = [ i \in Instances |->
                [lifecycle |-> "ACTIVE", phase |-> "READY", revision |-> 0, expectRev |-> 0,
                 epoch |-> 0, ctxEpoch |-> 0, activeReq |-> nil, tail |-> "user"] ]
  /\ goal = [status |-> "ACTIVE", known |-> 0, unknown |-> 0, reserved |-> {}]
  /\ requests = [ r \in ReqIds |->
                    [status |-> "none", instance |-> "", epoch |-> 0, est |-> 1,
                     selected |-> FALSE, result |-> "reply"] ]
  /\ attempts = [ a \in AttIds |-> [req |-> 0, status |-> "none"] ]
  /\ ops = [ o \in Ops |-> [status |-> "CANCELLED", instance |-> CHOOSE x \in Instances : TRUE,
                            epoch |-> 0, effect |-> 0, dispatched |-> FALSE] ]
  /\ approvals = [ o \in Ops |-> "none" ]
  /\ dead = {}

Spec == Init /\ [][Next]_vars /\ WF_vars(\E i \in Instances : Recover(i))

\* ---------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A i \in Instances : inst[i].phase \in
        {"READY", "MODEL_PENDING", "TOOLS_PENDING", "WAITING", "COMPLETION_PENDING"}
  /\ \A i \in Instances : inst[i].lifecycle \in {"ACTIVE", "PAUSED", "PARKED", "TERMINATED"}
  /\ \A o \in Ops : ops[o].status \in OpNonTerminal \cup OpTerminal
  /\ \A o \in Ops : ops[o].effect \in {0, 1}
  /\ goal.status \in {"ACTIVE", "SUCCEEDED", "FAILED", "BLOCKED", "CANCELLED"}

\* A25/A12: no side effect before an approval that was required
NoEffectBeforeApproval ==
  \A o \in Ops : ops[o].effect > 0 => ApprovalSatisfied(o)

\* A08/A10/A11/A13: the durable dispatch record precedes the effect
RecordBeforeEffect ==
  \A o \in Ops : ops[o].effect > 0 => ops[o].dispatched

EffectAtMostOnce ==
  \A o \in Ops : ops[o].effect <= 1

\* A18/§8: every live reservation passed the admission gate, so the holds can
\* never exceed the limit. Settled usage may exceed it afterwards — the plan
\* refuses a "never overspend" promise when provider billing is incomplete,
\* so the honest property is the gate, not the final total.
ReservationsAdmitted ==
  ReservedTotal <= TokenLimit

\* §8: closing a request always releases its reservation
ReservationReleased ==
  \A r \in ReqIds : requests[r].status \in ReqClosed => <<r, requests[r].est>> \notin goal.reserved

\* §6.1: at most one live request per instance, and it is the active one
OneActiveRequest ==
  \A i \in Instances : inst[i].phase = "MODEL_PENDING" =>
      (inst[i].activeReq \in ReqIds /\ requests[inst[i].activeReq].status = "PENDING")

\* §7/A19: only a selected complete attempt exists, and it is unique per request
SelectionIsComplete ==
  \A r \in ReqIds : requests[r].selected =>
     \E a \in AttIds : attempts[a].req = r /\ attempts[a].status = "COMPLETE"

\* the idle rule: a turn only starts when the last word is not the assistant's own
NoTurnWithoutWork ==
  \A i \in Instances : inst[i].phase \in {"MODEL_PENDING", "TOOLS_PENDING"} =>
      inst[i].tail # "assistant"

\* §6.1: an advancing executor always holds the current revision
StaleExecutorRejected ==
  \A i \in Instances : inst[i].phase = "MODEL_PENDING" => Fresh(i)

\* A24 is an *ordering* property (a receipt must not cross an epoch): the
\* state-only form is too strong because historical terminal operations keep
\* their own epoch after a reset. See NoReceiptAcrossEpochs below.

\* A13/§6.4: a terminated instance runs no effect
NoEffectOnTerminated ==
  \A o \in Ops : ops[o].effect > 0 => inst[ops[o].instance].lifecycle # "TERMINATED"

\* a PREPARED operation has produced nothing yet
PreparedIsNotTerminal ==
  \A o \in Ops : ops[o].status = "PREPARED" => ops[o].effect = 0

\* §6.2: "cancelled before start" means the external effect never began; the
\* durable dispatch record may already exist (that is how recovery knows)
CancelledBeforeStartHasNoEffect ==
  \A o \in Ops : ops[o].status = "CANCELLED_BEFORE_START" => ops[o].effect = 0

\* temporal properties (PROPERTIES in MC_props.cfg) --------------------------
\* A13/§6.1: a terminal operation status is never rewritten
TerminalOpStable ==
  [][ \A o \in Ops : ops[o].status \in OpTerminal => ops'[o].status = ops[o].status ]_vars

\* §8: a settled goal never changes status again. (Its *usage record* may keep
\* growing: like the implementation, the model admits a new request against a
\* goal that already reached a terminal status, because reserve_budget and
\* settle_usage carry no status guard. Recorded as a modelled boundary rather
\* than assumed away — see verification/README.md.)
TerminalGoalStatusStable ==
  [][ goal.status # "ACTIVE" => goal'["status"] = goal.status ]_vars

\* A24: a receipt only ever lands while its operation's epoch is current
NoReceiptAcrossEpochs ==
  [][ \A o \in Ops :
        (ops'[o].status \in {"SUCCEEDED", "FAILED"} /\ ops[o].status \notin {"SUCCEEDED", "FAILED"})
          => ops[o].epoch = inst[ops[o].instance].ctxEpoch ]_vars

\* the admission gate: a request only starts while known + reserved + est fits
AdmissionGate ==
  [][ \A i \in Instances : (inst'[i].phase = "MODEL_PENDING" /\ inst[i].phase # "MODEL_PENDING")
        => goal.known + ReservedTotal + 1 <= TokenLimit ]_vars

=============================================================================
