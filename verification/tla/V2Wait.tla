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
(* Two paths end a wait without the drain, both faithful to the code:        *)
(*  - satisfied at registration (the import evaluates in its own transaction, *)
(*    A23) — the code appends no answer on this path;                       *)
(*  - superseded by user input or an epoch close/reset, which sets the        *)
(*    instance's PENDING waits to CANCELLED — also with no answer.            *)
(* Both leave the wait's own tool_call unanswered. MC_wait_contract.cfg       *)
(* checks the intended "a resolved wait answers its call" contract and is     *)
(* expected to fail on exactly those two paths: that is the recorded evidence *)
(* for the open defect (see verification/README.md, finding V-W1).            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Waits,      \* wait slots (one row per registered wait), e.g. {"w1"}
          Conds,      \* conditions any wait may name, e.g. {"c1","c2"}
          Instances   \* instances that can park, e.g. {"L"}

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
  answerCall  \* wait slot -> the call id that answer used

vars == <<phase, waitState, waitMode, waitConds, waitCall, waitOwner,
          facts, timers, answers, answerCall>>

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
\* `answers` stays 0 — the code appends nothing on this path, and the instance
\* never parks when the wait is already satisfied.
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
     /\ answers' = [answers EXCEPT ![w] = 0]
     /\ answerCall' = [answerCall EXCEPT ![w] = nocall]
     /\ phase' = [phase EXCEPT ![i] = IF satisfied THEN "READY" ELSE "WAITING"]
  /\ UNCHANGED <<facts, timers>>

\* Environment: a message is delivered / a task or operation reaches a terminal
\* status. The code applies such facts through commands that sweep in the same
\* transaction (drain_inbox, complete_task, cancel_task, complete_operation),
\* so this step carries its wake. Facts that land without a sweep (a cancelled
\* operation, an expired approval) are closed by the parked drain instead.
FactAppears(c) ==
  /\ ~facts[c]
  /\ LET f == [facts EXCEPT ![c] = TRUE] IN
     /\ facts' = f
     /\ Sweep(f, timers)
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, timers>>

\* Environment: the clock reaches a wait's timer (fire_timer, poll granularity).
ClockTicks(w) ==
  /\ ~timers[w] /\ waitState[w] # "none"
  /\ LET t == [timers EXCEPT ![w] = TRUE] IN
     /\ timers' = t
     /\ Sweep(facts, t)
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, facts>>

\* The parked drain (driver §5.3/A23): every satisfiable pending wait of the
\* session closes in one transaction. Its guard is the poll loop's state; the
\* sweep is idempotent, so a replay changes nothing.
Drain(i) ==
  /\ phase[i] = "WAITING"
  /\ \E w \in Waits : waitOwner[w] = i /\ waitState[w] = "PENDING"
  /\ Sweep(facts, timers)
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, facts, timers>>

\* Supersede (§5.3/§5.4): user input, an epoch close or a reset cancels the
\* instance's pending waits and makes it runnable again. The pending wait stays
\* answered by nothing: the model keeps that observable behaviour.
Supersede(i) ==
  /\ phase[i] = "WAITING"
  /\ waitState' = [ w \in Waits |->
                     IF waitOwner[w] = i /\ waitState[w] = "PENDING" THEN "CANCELLED"
                       ELSE waitState[w] ]
  /\ phase' = [phase EXCEPT ![i] = "READY"]
  /\ UNCHANGED <<waitMode, waitConds, waitCall, waitOwner, facts, timers, answers, answerCall>>

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
  /\ UNCHANGED <<phase, facts, timers>>

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

\* no spurious wake: an answer exists only for a wait whose conditions held
AnswerImpliesConditions ==
  \A w \in Waits : answers[w] = 1 => Holds(w, facts, timers)

\* an answer is only ever appended together with SATISFIED
AnswerImpliesSatisfied ==
  \A w \in Waits : answers[w] = 1 => waitState[w] = "SATISFIED"

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

\* ------------------------------------------------------------------ properties --
\* A23 liveness: a pending wait whose conditions hold is closed — SATISFIED by
\* the parked drain, or CANCELLED when new input/epoch supersedes it. Nothing
\* stays pending forever while its conditions hold.
NoStrandedPending ==
  \A w \in Waits :
      []( (waitState[w] = "PENDING" /\ Holds(w, facts, timers)) => <>(waitState[w] \in Resolved) )

\* Intended contract, *not* a property of the current code: every resolved wait
\* answered the tool_call it came from (a strict wire endpoint rejects an
\* assistant call with no tool response). MC_wait_contract.cfg runs it to keep
\* the counterexample on file — see finding V-W1.
ResolvedWaitAnswersItsCall ==
  \A w \in Waits : (waitState[w] \in Resolved) => answers[w] = 1

=============================================================================
