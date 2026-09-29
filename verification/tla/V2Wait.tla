------------------------------ MODULE V2Wait ---------------------------------
(***************************************************************************)
(* Wait / wake / supersede model (plan §5.3, §5.4; acceptance A22/A23, RT-06).*)
(*                                                                          *)
(* Code anchors: core/src/v2/control.rs — the registration check inside      *)
(* import_response, evaluate_wait / condition_state, wake_satisfied_at       *)
(* (the sweep), submit_input (supersede), close_epoch_execution, fire_timer; *)
(* engine/src/v2/driver.rs — the WAITING drains, the only wake path while a  *)
(* instance is parked (weakly fair: the driver polls).                       *)
(*                                                                          *)
(* Facts are *monotone* here: a delivered message stays applied, a task stays *)
(* terminal, a fired timer stays due — which is why "the condition held at   *)
(* some point" is checkable as a state predicate.                            *)
(*                                                                          *)
(* Two paths end a wait without the drain, and both answer its tool call      *)
(* (§5.3/A23; finding V-W1, fixed in the code and guarded by the regression    *)
(* test `wait_call_answered_outside_the_drain_path`):                         *)
(*  - satisfied at registration (the import evaluates in its own transaction, *)
(*    A23) — answered with the same wake reason the drain uses;               *)
(*  - superseded by user input or an epoch close/reset, which sets the         *)
(*    instance's PENDING waits to CANCELLED and answers the cancellation.      *)
(* `ResolvedWaitIsAnswered` is the invariant that keeps both honest.           *)
(*                                                                          *)
(* Counterfactual constant (D-220): `CloseWithoutAnswer` TRUE restores exactly *)
(* the pre-V-W1 shape — both non-drain exits close the wait and answer         *)
(* nothing — which is the refutation the fix ledger records for that finding    *)
(* and which no configuration had carried since its old one was renamed into    *)
(* the positive `MC_wait.cfg`.                                                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Waits,      \* wait slots (one row per registered wait), e.g. {"w1"}
          Conds,      \* conditions any wait may name, e.g. {"c1","c2"}
          Instances,  \* instances that can park, e.g. {"L"}
          CloseWithoutAnswer, \* counterfactual (D-220): the two non-drain exits close a wait without answering it
          TaskSettled,        \* the condition standing for a delegated task's settlement (a task result)
          MessageCond,        \* the condition standing for a chat message from a sender
          WidenMessageToTaskResults, \* D-255(a)/D-341: a runtime that lets a task result satisfy a message condition
          WakeOnReport,              \* D-255(b)/D-341: a runtime that treats a reported BLOCKED as a settlement
          WakeOnIdle                 \* D-257/D-341: a runtime that wakes a delegator whose assignee went idle

ASSUME Waits # {} /\ Conds # {} /\ Instances # {}

Modes == {"ALL", "ANY"}
nocall == "none"         \* sentinel: the wait carried no tool_call to answer
WaitStatus == {"none", "PENDING", "SATISFIED", "CANCELLED"}
Resolved == {"SATISFIED", "CANCELLED"}
Phases == {"READY", "WAITING"}

VARIABLES
  phase,      \* instance -> READY | WAITING
  waitState,  \* wait slot -> status
  waitMode,   \* wait slot -> mode chosen at registration
  waitConds,  \* wait slot -> the conditions it requires
  waitCall,   \* wait slot -> the tool_call id to answer (nocall = none)
  waitOwner,  \* wait slot -> owning instance
  facts,      \* condition -> whether its fact already happened (monotone)
  timers,     \* wait slot -> whether its timer already fired (monotone)
  answers,    \* wait slot -> 1 once a wake answer joined the context
  answerCall, \* wait slot -> the call id that answer used
  reported,   \* the delegated task was reported BLOCKED (a report, not a settlement)
  idle        \* the assignee ended its turn without settling (it went idle)

vars == <<phase, waitState, waitMode, waitConds, waitCall, waitOwner,
          facts, timers, answers, answerCall, reported, idle>>

\* ------------------------------------------- evaluation at a given fact set --
\* The mode rule of §5.3: ALL = every named fact (or the due timer), ANY = one
\* named fact (or the due timer). Facts/timers are passed in so that a fact can
\* be applied and the wake it causes be evaluated in the same step.
HoldsWith(mode, conds, w, f, t) ==
  LET held == { c \in conds : f[c] } IN
  \/ (mode = "ALL" /\ conds \subseteq held)
  \/ (mode = "ANY" /\ held # {})
  \/ t[w]

Holds(w, f, t) == HoldsWith(waitMode[w], waitConds[w], w, f, t)

\* pending and closable by a sweep over (f, t)
Closable(w, f, t) == waitState[w] = "PENDING" /\ Holds(w, f, t)

\* what such a sweep leaves pending
StaysPending(w, f, t) == waitState[w] = "PENDING" /\ ~Holds(w, f, t)

\* the code's rule for a woken instance: WAITING -> READY once nothing of its
\* own stays pending; every other phase is untouched
PhaseAfter(f, t) ==
  [ x \in Instances |->
      IF phase[x] = "WAITING"
         /\ (\A w \in Waits : ~(waitOwner[w] = x /\ StaysPending(w, f, t)))
        THEN "READY" ELSE phase[x] ]

\* one sweep (wake_satisfied_at): every closable pending wait closes with its
\* answer in the same transaction; a replay is a no-op because a closed wait is
\* no longer PENDING
Sweep(f, t) ==
  /\ waitState' = [ w \in Waits |-> IF Closable(w, f, t) THEN "SATISFIED" ELSE waitState[w] ]
  /\ answers' = [ w \in Waits |-> IF Closable(w, f, t) THEN 1 ELSE answers[w] ]
  /\ answerCall' = [ w \in Waits |-> IF Closable(w, f, t) THEN waitCall[w] ELSE answerCall[w] ]
  /\ phase' = PhaseAfter(f, t)

\* ------------------------------------------------------------------- actions --
\* Registration (§5.3/A23): the wait is checked against the facts that already
\* hold in the same transaction, so a result that arrived first is never lost.
\* A wait satisfied right here is answered right here, in the same transaction,
\* and the instance never parks.
ArmWait(w, i, mode, conds, call) ==
  /\ waitState[w] = "none" /\ phase[i] = "READY"
  /\ mode \in Modes /\ conds \subseteq Conds /\ conds # {}
  /\ call \in {nocall} \cup Waits
  /\ waitMode' = [waitMode EXCEPT ![w] = mode]
  /\ waitConds' = [waitConds EXCEPT ![w] = conds]
  /\ waitCall' = [waitCall EXCEPT ![w] = call]
  /\ waitOwner' = [waitOwner EXCEPT ![w] = i]
  /\ LET satisfied == HoldsWith(mode, conds, w, facts, timers) IN
     /\ waitState' = [waitState EXCEPT ![w] = IF satisfied THEN "SATISFIED" ELSE "PENDING"]
     /\ answers' = [answers EXCEPT ![w] = IF satisfied /\ ~CloseWithoutAnswer THEN 1 ELSE 0]
     /\ answerCall' = [answerCall EXCEPT ![w] = IF satisfied /\ ~CloseWithoutAnswer THEN call ELSE nocall]
     /\ phase' = [phase EXCEPT ![i] = IF satisfied THEN "READY" ELSE "WAITING"]
  /\ UNCHANGED <<facts, timers, reported, idle>>

\* Environment: a message is delivered / a task or operation reaches a terminal
\* status. The code applies such facts through commands that sweep in the same
\* transaction (drain_inbox, complete_task, cancel_task, complete_operation),
\* so this step carries its wake. Facts that land without a sweep (a cancelled
\* operation, an expired approval) are closed by the parked drain instead.
FactAppears(c) ==
  /\ ~facts[c]
  /\ LET f == IF c = TaskSettled /\ WidenMessageToTaskResults
                THEN [facts EXCEPT ![TaskSettled] = TRUE, ![MessageCond] = TRUE]
                ELSE [facts EXCEPT ![c] = TRUE] IN
     /\ facts' = f
     /\ Sweep(f, timers)
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, timers, reported, idle>>

\* D-255(b)/D-341: the assignee settles the task BLOCKED. That is a *report* — the assignee may still be
\* unblocked and settle the same task later — so it is no fact and wakes nobody; `WakeOnReport` is the
\* counterfactual in which the old question was answered "yes".
ReportBlocked ==
  /\ ~reported
  /\ LET f == IF WakeOnReport THEN [facts EXCEPT ![TaskSettled] = TRUE] ELSE facts IN
     /\ reported' = TRUE
     /\ facts' = f
     /\ IF WakeOnReport THEN Sweep(f, timers) ELSE UNCHANGED <<waitState, phase, answers, answerCall>>
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, timers, idle>>

\* D-257/D-341: the assignee ends its turn without settling, then goes idle. An idle assignee leaves its task
\* open: no fact appears and the wait stays pending — `WakeOnIdle` is the counterfactual answer.
AssigneeGoesIdle ==
  /\ ~idle
  /\ LET f == IF WakeOnIdle THEN [facts EXCEPT ![TaskSettled] = TRUE] ELSE facts IN
     /\ idle' = TRUE
     /\ facts' = f
     /\ IF WakeOnIdle THEN Sweep(f, timers) ELSE UNCHANGED <<waitState, phase, answers, answerCall>>
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, timers, reported>>

\* Environment: the clock reaches a wait's timer (fire_timer, poll granularity).
ClockTicks(w) ==
  /\ ~timers[w] /\ waitState[w] # "none"
  /\ LET t == [timers EXCEPT ![w] = TRUE] IN
     /\ timers' = t
     /\ Sweep(facts, t)
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, facts, reported, idle>>

\* The parked drain (driver §5.3/A23): every satisfiable pending wait of the
\* session closes in one transaction. Its guard is the poll loop's state; the
\* sweep is idempotent, so a replay changes nothing.
Drain(i) ==
  /\ phase[i] = "WAITING"
  /\ \E w \in Waits : waitOwner[w] = i /\ waitState[w] = "PENDING"
  /\ Sweep(facts, timers)
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, facts, timers, reported, idle>>

\* Supersede (§5.3/§5.4): user input, an epoch close or a reset cancels the
\* instance's pending waits, answers each of them (a cancelled wait never
\* reaches the drain, so its call would otherwise stay unanswered) and makes
\* the instance runnable again.
Supersede(i) ==
  /\ phase[i] = "WAITING"
  /\ LET cancelled == { w \in Waits : waitOwner[w] = i /\ waitState[w] = "PENDING" } IN
     /\ waitState' = [ w \in Waits |-> IF w \in cancelled THEN "CANCELLED" ELSE waitState[w] ]
     /\ answers' = [ w \in Waits |-> IF w \in cancelled /\ ~CloseWithoutAnswer THEN 1 ELSE answers[w] ]
     /\ answerCall' = [ w \in Waits |->
                          IF w \in cancelled /\ ~CloseWithoutAnswer THEN waitCall[w] ELSE answerCall[w] ]
  /\ phase' = [phase EXCEPT ![i] = "READY"]
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, facts, timers, reported, idle>>

\* A new wait row replaces a resolved one (the code keeps one row per decision):
\* the slot is reusable only while its instance is not parked on it.
Retire(w) ==
  /\ waitState[w] \in Resolved
  /\ phase[waitOwner[w]] = "READY"
  /\ waitState' = [waitState EXCEPT ![w] = "none"]
  /\ waitMode' = [waitMode EXCEPT ![w] = "ALL"]
  /\ waitConds' = [waitConds EXCEPT ![w] = {}]
  /\ waitCall' = [waitCall EXCEPT ![w] = nocall]
  /\ waitOwner' = [waitOwner EXCEPT ![w] = "none"]
  /\ answers' = [answers EXCEPT ![w] = 0]
  /\ answerCall' = [answerCall EXCEPT ![w] = nocall]
  /\ UNCHANGED <<phase, facts, timers, reported, idle>>

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED vars

Next ==
  \/ \E w \in Waits : \E i \in Instances : \E mode \in Modes :
        \E conds \in SUBSET Conds : \E call \in {nocall} \cup Waits :
           /\ conds # {} /\ ArmWait(w, i, mode, conds, call)
  \/ \E c \in Conds : FactAppears(c)
  \/ \E w \in Waits : ClockTicks(w)
  \/ \E i \in Instances : Drain(i)
  \/ \E i \in Instances : Supersede(i)
  \/ \E w \in Waits : Retire(w)
  \/ ReportBlocked
  \/ AssigneeGoesIdle
  \/ Stutter

Init ==
  /\ phase = [ i \in Instances |-> "READY" ]
  /\ waitState = [ w \in Waits |-> "none" ]
  /\ waitMode = [ w \in Waits |-> "ALL" ]
  /\ waitConds = [ w \in Waits |-> {} ]
  /\ waitCall = [ w \in Waits |-> nocall ]
  /\ waitOwner = [ w \in Waits |-> "none" ]
  /\ facts = [ c \in Conds |-> FALSE ]
  /\ timers = [ w \in Waits |-> FALSE ]
  /\ answers = [ w \in Waits |-> 0 ]
  /\ answerCall = [ w \in Waits |-> nocall ]
  /\ reported = FALSE
  /\ idle = FALSE

\* The parked drain is the wait's liveness source: while an instance is WAITING
\* with a pending wait, the driver keeps sweeping (engine/v2/driver.rs), so that
\* sweep is weakly fair. A supersede may close the wait as CANCELLED first.
Spec == Init /\ [][Next]_vars /\ WF_vars(\E i \in Instances : Drain(i))

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A i \in Instances : phase[i] \in Phases
  /\ \A w \in Waits : waitState[w] \in WaitStatus
  /\ \A w \in Waits : answers[w] \in {0, 1}

\* the wake answer joins the context at most once per wait (RT-06 style dedup)
WakeAnswerAtMostOnce ==
  \A w \in Waits : answers[w] <= 1

\* no spurious wake: a SATISFIED wait really had its conditions hold (the
\* answer of a CANCELLED wait is a cancellation notice, not a wake)
SatisfiedHoldsConditions ==
  \A w \in Waits : waitState[w] = "SATISFIED" => Holds(w, facts, timers)

\* finding V-W1: every wait that ended is answered, whatever ended it — the
\* drain wake, a registration that was already satisfied, or a supersede. A
\* strict wire endpoint rejects a request whose assistant tool_calls have no
\* matching tool response, so an unanswered control call is not an option.
ResolvedWaitIsAnswered ==
  \A w \in Waits : waitState[w] \in Resolved => answers[w] = 1

\* an answer is only ever appended together with a resolved wait
AnswerImpliesResolved ==
  \A w \in Waits : answers[w] = 1 => waitState[w] \in Resolved

\* the answer names the wait's own tool_call when it had one (R22 pairing fix)
WakeAnswersItsCall ==
  \A w \in Waits : answers[w] = 1 /\ waitCall[w] # nocall => answerCall[w] = waitCall[w]

\* a parked instance always has a pending wait of its own to be woken by
WaitingHasPendingWait ==
  \A i \in Instances : phase[i] = "WAITING" =>
      \E w \in Waits : waitOwner[w] = i /\ waitState[w] = "PENDING"

\* every pending wait belongs to a parked instance: the parked drain is enabled,
\* so no wait can be left pending while its owner runs on
PendingImpliesParked ==
  \A w \in Waits : waitState[w] = "PENDING" => phase[waitOwner[w]] = "WAITING"

\* an unset slot was never answered
UnusedSlotHasNoAnswer ==
  \A w \in Waits : waitState[w] = "none" => answers[w] = 0

\* ---------------------------------------------------------------- properties --
\* The three rules the user decided on 2026-09-29 (D-341), written into DESIGN §5.3 by D-348. Each is a step
\* property: the step that produces the fact must be the *right* step.
\* (a) a task's settlement is a task fact: it never satisfies a message condition.
MessageStaysAMessage ==
  [][ (facts[TaskSettled]' /\ ~facts[TaskSettled]) => facts[MessageCond]' = facts[MessageCond] ]_vars

\* (b) a reported BLOCKED is a report, not a settlement.
ReportsAreNotSettlements ==
  [][ (reported' /\ ~reported) => facts[TaskSettled]' = facts[TaskSettled] ]_vars

\* (c) an assignee that goes idle settles nothing, so the wait stays pending (`NoStrandedPending` is the
\* liveness half and `SatisfiedHoldsConditions` keeps the answer honest).
IdleIsNotASettlement ==
  [][ (idle' /\ ~idle) => facts[TaskSettled]' = facts[TaskSettled] ]_vars
\* A23 liveness: a pending wait whose conditions hold is closed — SATISFIED by
\* the parked drain, or CANCELLED when new input/epoch supersedes it. Nothing
\* stays pending forever while its conditions hold.
NoStrandedPending ==
  \A w \in Waits :
      []( (waitState[w] = "PENDING" /\ Holds(w, facts, timers)) => <>(waitState[w] \in Resolved) )

=============================================================================
