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
          AllowMidTurnInput, \* counterfactual: apply input while a request is in flight (pre-D-63)
          PerInstanceFairness, \* counterfactual: fairness as one disjunction over instances (pre-D-63)
          IgnoreDeadline,  \* counterfactual: a runtime that ignores the goal deadline (A35)
          ReleaseOnCancel, \* counterfactual: a runtime that closes a spent goal and leaves the instance its
                           \* refusal parked where it was (D-341/D-344)
          ReaskAfterReply, \* counterfactual: re-open a turn when the last word is the model's own (D-65)
          RuntimeTailIsWork, \* counterfactual: treat the runtime's own closing note as unaddressed (D-71)
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
             \*               activeReq, ctxEpoch, tail, queue, inputMidTurn]
             \*               inputMidTurn: a user input landed while a turn was in
             \*               flight (monitor for `InputLandsAtTheBoundary`, D-63)
  goal,      \* [status, known, unknown, reserved, deadlinePassed]
             \* deadlinePassed: the goal's wall-clock deadline has passed (a fixed
             \* timestamp in the code; monotone here) — past it no new request begins
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

\* The instance's last committed word: the model's own text ("assistant") or the
\* runtime's own closing note ("runtime", §8/D-71 — a settlement or a closed turn
\* the runtime states in its own voice). Neither awaits an answer; a tail in
\* UnaddressedTails is work the model has not seen yet.
CommittedTails   == {"assistant", "runtime"}
UnaddressedTails == {"user", "tool"} \cup (IF RuntimeTailIsWork THEN {"runtime"} ELSE {})

\* ------------------------------------------------------------------- actions --
\* user input lands at the READY boundary (§5.4, and the model of the driver's
\* drain): an input that arrives while a request is in flight cannot be part of
\* that fixed request, so it waits in the inbox (D-63)
Input(i) ==
  /\ Alive(i) /\ inst[i].phase = "READY" /\ inst[i].lifecycle = "ACTIVE"
  /\ inst' = [inst EXCEPT ![i].tail = "user", ![i].queue = FALSE,
                              ![i].afterLanding = FALSE]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* the driver's boundary: apply the queued input once the turn in flight ended.
\* Weak fairness on this action is the liveness assumption behind
\* `QueuedInputEntersTheContext` below (the drain runs at every boundary).
ApplyQueued(i) ==
  /\ Alive(i) /\ inst[i].queue /\ inst[i].phase = "READY" /\ inst[i].lifecycle = "ACTIVE"
  /\ inst' = [inst EXCEPT ![i].tail = "user", ![i].queue = FALSE,
                              ![i].afterLanding = FALSE]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* the user submits while a walk is in flight: the input waits for the boundary
QueueInput(i) ==
  /\ Alive(i) /\ inst[i].lifecycle = "ACTIVE"
  /\ inst[i].phase \in {"MODEL_PENDING", "TOOLS_PENDING", "COMPLETION_PENDING"}
  /\ inst' = [inst EXCEPT ![i].queue = TRUE]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* Counterfactual (AllowMidTurnInput = TRUE): the pre-D-63 code stored the input
\* in the running turn's context. The turn's own reply then lands *after* it, so
\* the model sees its answer as the answer to an input it never saw and the idle
\* rule gives the input no turn — the input is lost. The negative control config
\* must refute `InputLandsAtTheBoundary` with this enabled.
MidTurnInput(i) ==
  /\ AllowMidTurnInput
  /\ Alive(i) /\ inst[i].lifecycle = "ACTIVE" /\ inst[i].phase # "READY"
  /\ inst' = [inst EXCEPT ![i].tail = "user", ![i].inputMidTurn = TRUE,
                              ![i].afterLanding = FALSE]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* the wall clock passes the goal's deadline (a fixed timestamp in the code; the
\* model only needs the monotone fact that it has passed)
DeadlinePasses ==
  /\ ~goal.deadlinePassed
  /\ goal' = [goal EXCEPT !.deadlinePassed = TRUE]
  /\ UNCHANGED <<inst, requests, attempts, ops, approvals, dead>>

\* READY -> MODEL_PENDING: revision guard, idle rule, budget reservation
BeginRequest(i) ==
  /\ Alive(i) /\ inst[i].phase = "READY" /\ inst[i].lifecycle = "ACTIVE"
  /\ Fresh(i)
  \* The idle rule: a turn opens for *unaddressed* work only. The counterfactual
  \* re-opens one while the last word is the model's own text — what the code did
  \* whenever an instance still had an open task (D-65), which is a turn storm on a
  \* model that answers with prose. The runtime's own closing note is the other
  \* committed tail: a runtime that asked the model to answer it would be inventing
  \* work out of its own settlement (D-71), which `RuntimeTailIsWork` states and
  \* `NoTurnWithoutWork` must refute.
  /\ (ReaskAfterReply \/ inst[i].tail \in UnaddressedTails)
  /\ ~inst[i].queue                      \* the boundary applies the queued input first
  /\ BudgetFits(1)
  \* A35: past the goal's deadline no new request begins. The counterfactual drops
  \* only this clause, so `NoRequestAfterDeadline` is sensitive to exactly it.
  /\ (IgnoreDeadline \/ ~goal.deadlinePassed)
  /\ \E r \in FreeReqs :
       /\ requests' = [requests EXCEPT ![r] = [status |-> "PENDING", instance |-> i,
                                              epoch |-> inst[i].epoch, est |-> 1,
                                              selected |-> FALSE, result |-> "reply"]]
       /\ goal' = [goal EXCEPT !.reserved = @ \cup {<<r, 1>>}]
       /\ inst' = [inst EXCEPT ![i].phase = "MODEL_PENDING", ![i].activeReq = r,
                                  ![i].revision = @ + 1, ![i].expectRev = @ + 1,
                                  \* a request of i's own is now addressing whatever
                                  \* landed last (a boundary drained the queue first:
                                  \* `~inst[i].queue` above)
                                  ![i].afterLanding = TRUE]
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

\* goal settlement happens once and only from ACTIVE (§8). The settlement leaves
\* the runtime's own closing note as the instance's last word: the runtime states
\* the ending, the model does not have to answer for it, and the idle rule
\* therefore leaves the instance alone (D-71). It also *records* whether the turn
\* that produced the completion followed the instance's last input landing — the
\* fact a client's outcome attribution rests on (D-72).
\* `tail = "assistant"` is the model's abstraction for "the model's own response is
\* what this settlement is about": in the code both settlement paths read a
\* completion decision of a request (`complete_goal`'s candidate, the check round's
\* finish), so a settlement cannot be produced out of thin air or by an input alone.
SettleGoal(i, status) ==
  \* CANCELLED is not in this set: the runtime settles what it concluded (SUCCEEDED/FAILED/BLOCKED), while
  \* CANCELLED comes only from the user's own cancel (CancelGoal), which is what the code does (complete_goal
  \* and block_goal never write it).
  /\ Alive(i) /\ status \in {"SUCCEEDED", "FAILED", "BLOCKED"}
  /\ goal.status = "ACTIVE"
  /\ inst[i].tail = "assistant"
  /\ NonTerminalOps(i) = {}                       \* no open operation survives a close
  /\ ~(\E r \in UsedReqs : requests[r].instance = i /\ requests[r].status = "PENDING")
  /\ goal' = [goal EXCEPT !.status = status]
  /\ inst' = [inst EXCEPT ![i].phase = "READY", ![i].tail = "runtime",
                              ![i].settledAfterTurn = inst[i].afterLanding]
  /\ UNCHANGED <<requests, attempts, ops, approvals, dead>>

\* D-341/D-344: the user's lever on a goal nothing can spend. A goal whose ceiling or deadline is gone stays
\* ACTIVE for ever, refusing new work (A18's gate) and still listed (A18's known gap); the instance its refusal
\* parked has no path back. This is the user's cancel: the goal goes terminal and the instances the refusal
\* parked are released, so the session keeps its workers. `set_lifecycle` keeps resume with the user, so this
\* does too — and `ReleaseOnCancel = FALSE` is the counterfactual that closes the goal and leaves them parked.
CancelGoal ==
  /\ goal.status = "ACTIVE"
  /\ \A i \in Instances : inst[i].activeReq = nil           \* no running work
  /\ \A r \in UsedReqs : requests[r].status # "PENDING"
  /\ \A o \in Ops : ops[o].status \notin OpNonTerminal
  /\ goal' = [goal EXCEPT !.status = "CANCELLED"]
  /\ inst' = [ j \in Instances |->
                 IF ReleaseOnCancel /\ inst[j].lifecycle = "PARKED"
                   THEN [inst[j] EXCEPT !.lifecycle = "ACTIVE"]
                   ELSE inst[j] ]
  /\ UNCHANGED <<requests, attempts, ops, approvals, dead>>

\* reset: new epoch closes the old execution, reservations released (A24)
ResetInstance(i) ==
  /\ Alive(i)
  /\ inst[i].epoch < MaxEpoch
  /\ LET newEpoch == inst[i].epoch + 1 IN
       \* a reset seals the old epoch's inbox (A24) and with it a queued input
       /\ inst' = [inst EXCEPT ![i].epoch = newEpoch, ![i].phase = "READY",
                              ![i].ctxEpoch = newEpoch, ![i].activeReq = nil,
                              ![i].queue = FALSE,
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
  \* a terminated instance takes no input: a queued one is not deliverable any more
  /\ inst' = [inst EXCEPT ![i].lifecycle = l,
                           ![i].queue = IF l = "TERMINATED" THEN FALSE ELSE inst[i].queue]
  /\ UNCHANGED <<goal, requests, attempts, ops, approvals, dead>>

\* ---------------------------------------------------------------------- spec --
\* an idle step keeps the model open-ended (TLC then reports no false deadlock)
Stutter == UNCHANGED vars

Next ==
  \/ DeadlinePasses
  \/ \E i \in Instances : Input(i)
  \/ \E i \in Instances : QueueInput(i)
  \/ \E i \in Instances : ApplyQueued(i)
  \/ \E i \in Instances : MidTurnInput(i)
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
  \/ \E i \in Instances : SettleGoal(i, CHOOSE x \in {"SUCCEEDED", "FAILED", "BLOCKED"} : TRUE)
  \/ CancelGoal
  \/ \E i \in Instances : ResetInstance(i)
  \/ \E i \in Instances : \E l \in {"ACTIVE", "PAUSED", "PARKED", "TERMINATED"} : SetLifecycle(i, l)
  \/ Stutter   \* the system is open-ended: an idle step is always possible

Init ==
  /\ inst = [ i \in Instances |->
                [lifecycle |-> "ACTIVE", phase |-> "READY", revision |-> 0, expectRev |-> 0,
                 epoch |-> 0, ctxEpoch |-> 0, activeReq |-> nil, tail |-> "user",
                 queue |-> FALSE, inputMidTurn |-> FALSE,
                 \* D-72 monitors: whether a request has begun since the last input
                 \* landing (a landing means the instance must address it with a
                 \* request of its own — the state begins with the boot input
                 \* already addressed, so this starts TRUE), and what that was
                 \* worth at settlement time (the invariant below reads it).
                 afterLanding |-> TRUE, settledAfterTurn |-> TRUE] ]
  /\ goal = [status |-> "ACTIVE", known |-> 0, unknown |-> 0, reserved |-> {}, deadlinePassed |-> FALSE]
  /\ requests = [ r \in ReqIds |->
                    [status |-> "none", instance |-> "", epoch |-> 0, est |-> 1,
                     selected |-> FALSE, result |-> "reply"] ]
  /\ attempts = [ a \in AttIds |-> [req |-> 0, status |-> "none"] ]
  /\ ops = [ o \in Ops |-> [status |-> "CANCELLED", instance |-> CHOOSE x \in Instances : TRUE,
                            epoch |-> 0, effect |-> 0, dispatched |-> FALSE] ]
  /\ approvals = [ o \in Ops |-> "none" ]
  /\ dead = {}

\* The turn advances: at least one step an instance's in-flight turn can take next.
\* Weak fairness on it is the assumption behind `QueuedInputEntersTheContext` — the
\* driver keeps driving, and the provider eventually answers or the transport times
\* out into `FailRequest` — so a turn in flight does not stay in flight forever.
\* (Approval waits are covered too: `FailRequest` is one of the steps.)
TurnStep(i) ==
  \/ RecordAttempt(i) \/ LostAttempt(i) \/ ImportResponse(i) \/ ConsumeDecision(i)
  \/ FailRequest(i) \/ CancelRequest(i)
  \/ \E o \in Ops : ops[o].instance = i /\
        (RequestApproval(o) \/ Dispatch(o) \/ StartEffect(o) \/ \E x \in OpTerminal : Complete(o, x))

\* Fairness is per instance, because each instance has its own driver: one
\* instance's progress must not be carried by another's (a single disjunction over
\* instances would let a two-instance configuration starve one of them, which the
\* two-instance run demonstrates).
\* Recovery, the boundary drain and turn progress are *per instance* (the supervisor
\* drives and restarts drivers instance by instance), so one instance cannot be
\* starved by another's progress. Stated as a switch because the older disjunction
\* form looks equivalent and is not: with two instances it lets one of them stay
\* dead while the other recovers, which the two-instance configuration refutes.
\* The drain is *strongly* fair: a driver that keeps running drains its inbox even if
\* the environment crashes it again and again (weak fairness alone lets a crash loop
\* starve it).
Spec == Init /\ [][Next]_vars
       /\ (IF PerInstanceFairness
             THEN (\A i \in Instances : WF_vars(Recover(i)))
             ELSE WF_vars(\E i \in Instances : Recover(i)))
       /\ (IF PerInstanceFairness
             THEN (\A i \in Instances : SF_vars(ApplyQueued(i)))
             ELSE SF_vars(\E i \in Instances : ApplyQueued(i)))
       /\ (IF PerInstanceFairness
             THEN (\A i \in Instances : WF_vars(TurnStep(i)))
             ELSE WF_vars(\E i \in Instances : TurnStep(i)))

\* ---------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A i \in Instances : inst[i].phase \in
        {"READY", "MODEL_PENDING", "TOOLS_PENDING", "WAITING", "COMPLETION_PENDING"}
  /\ \A i \in Instances : inst[i].lifecycle \in {"ACTIVE", "PAUSED", "PARKED", "TERMINATED"}
  /\ \A i \in Instances : inst[i].queue \in BOOLEAN
  /\ \A i \in Instances : inst[i].inputMidTurn \in BOOLEAN
  /\ \A i \in Instances : inst[i].afterLanding \in BOOLEAN
  /\ \A i \in Instances : inst[i].settledAfterTurn \in BOOLEAN
  /\ \A o \in Ops : ops[o].status \in OpNonTerminal \cup OpTerminal
  /\ \A o \in Ops : ops[o].effect \in {0, 1}
  /\ goal.status \in {"ACTIVE", "SUCCEEDED", "FAILED", "BLOCKED", "CANCELLED"}
  /\ goal.deadlinePassed \in BOOLEAN

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

\* the idle rule: a turn only starts when the last word is not already committed —
\* neither the model's own text nor the runtime's own closing note (§8, D-71)
NoTurnWithoutWork ==
  \A i \in Instances : inst[i].phase \in {"MODEL_PENDING", "TOOLS_PENDING"} =>
      inst[i].tail \notin CommittedTails

\* D-72: an outcome belongs to the turn that produced it. A settlement must follow
\* a request that began *after* the instance's last input landing — because a
\* landing puts the input in the context the next request is fixed from, the
\* session-level outcome a client sees after its own input landed is its input's
\* outcome, and one recorded before that belongs to another turn.
\*
\* The counterfactual `AllowMidTurnInput` (the pre-D-63 code, refuted by D-63's own
\* property) refutes this too: an input stored *inside* a running turn is not in
\* that turn's fixed request, yet the turn's completion still settles the goal — an
\* outcome a waiting client would attribute to an input the model never saw.
SettlementFollowsATurnAfterTheLanding ==
  \A i \in Instances : inst[i].settledAfterTurn

\* D-63: user input only ever enters the context at a READY boundary. The monitor
\* records the phase each landing happened at; the counterfactual where the driver
\* applied it inside the running turn (AllowMidTurnInput) refutes this — and loses
\* the input, because that turn's own reply is appended after it.
InputLandsAtTheBoundary ==
  \A i \in Instances : ~inst[i].inputMidTurn

\* D-63: an input that waited for the boundary enters the context, or the instance
\* stops being active (parked/terminated: the user resumes it) or a reset seals it
\* with its epoch (A24). It is never dropped while the instance keeps running.
\* Proved under the two fairness assumptions above (the drain runs at a boundary,
\* and a turn in flight eventually ends).
QueuedInputEntersTheContext ==
  \A i \in Instances :
    []( (inst[i].queue /\ inst[i].lifecycle = "ACTIVE")
        => <>(inst[i].tail = "user" \/ ~inst[i].queue \/ inst[i].lifecycle # "ACTIVE") )

\* A35: a goal that is past its deadline begins no new request (the gate in
\* `begin_request`/`begin_compression`). The refusal itself parks the instance
\* through the same classified path `FailRequest` models; this property is the
\* gate, which is what the configured ceiling promises.
NoRequestAfterDeadline ==
  [][ \A i \in Instances : (inst[i].phase # "MODEL_PENDING" /\ inst'[i].phase = "MODEL_PENDING") =>
        ~goal.deadlinePassed ]_vars

\* D-341/D-344: closing a goal nothing can spend gives the session its workers back — the step that makes the
\* goal CANCELLED may not leave an instance parked behind it. Every positive config sets `ReleaseOnCancel =
\* TRUE`; the control that drops it must be refuted. (One goal per session in this model, so the release is
\* stated over every instance; the code releases the instances that goal\'s refusal parked.)
CancelledGoalReleasesParkedInstances ==
  [][ (goal.status # "CANCELLED" /\ goal' = [goal EXCEPT !.status = "CANCELLED"])
      => \A i \in Instances : inst'[i].lifecycle # "PARKED" ]_vars

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
