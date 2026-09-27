------------------------------ MODULE V2Approval -----------------------------
(***************************************************************************)
(* The approval window of a gated operation (plan §6.1/§9; acceptance A25, A08). *)
(*                                                                          *)
(* Code anchors: core/src/v2/control.rs — a gated operation *parks* behind a    *)
(* PENDING approval instead of dispatching (`park_operation`), `change_approval` *)
(* flips a PENDING row and may carry a new `expires_at`, the dispatch re-check  *)
(* requires status APPROVED with a matching args hash and revision and         *)
(* `expires_at IS NULL OR expires_at > now`, and `expire_pending_approvals`     *)
(* runs when the operation closes, so a terminal operation leaves no pending    *)
(* approval behind (the regression `pending_approvals_expire_when_the_operation_*)
(* closes`).                                                                   *)
(*                                                                          *)
(* Abstraction: one approval per operation (the code looks exactly one up by    *)
(* `operation_id`); the args hash and the grant revision belong to the dispatch *)
(* re-check and are modelled in V2Grants; "now" is the clock, and the status    *)
(* only becomes EXPIRED through the closing cascade, because that is the only   *)
(* place the code writes it. What is left is the window itself: an approval is  *)
(* viable only while its horizon is ahead, a decision is final, and an          *)
(* operation that closes takes its pending approval with it.                    *)
(*                                                                          *)
(* Counterfactual constants (D-225): each one TRUE is a plausible mistake the   *)
(* claims must refute, so they are falsifiable rather than merely stated —      *)
(* `DropsExpiryCheck` dispatches past the horizon, `RewritesDecisions` rewrites *)
(* a decided approval, and `KeepsPendingOnClose` leaves a closing operation's   *)
(* approval pending.                                                            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Ops,             \* operation slots, e.g. {"o1","o2"}
          MaxTime,         \* the clock's bound (keeps the state space finite)
          DropsExpiryCheck,    \* counterfactual: dispatch ignores the horizon
          RewritesDecisions,   \* counterfactual: a decided approval is rewritten
          KeepsPendingOnClose, \* counterfactual: a closing operation leaves its approval pending
          ParksWithoutARow     \* counterfactual: parking the operation creates no approval row

ASSUME Ops # {} /\ MaxTime > 0

ApprStatus == {"none", "PENDING", "APPROVED", "DENIED", "EXPIRED"}
Decided == {"APPROVED", "DENIED", "EXPIRED"}
OpStatus == {"none", "PARKED", "CLOSED"}
nohorizon == 0 - 1     \* sentinel for "the approval carries no expiry" (`expires_at IS NULL`)

VARIABLES
  apprStatus,  \* operation -> its approval's status
  horizon,     \* operation -> the moment its approval lapses (nohorizon = none)
  opStatus,    \* operation -> none | PARKED (waiting on the gate) | CLOSED
  effect,      \* operation -> whether its effect landed (0 or 1)
  clock,       \* the session clock
  lateEffect,  \* monitor: an effect landed with the horizon already reached (must stay FALSE)
  rewritten    \* monitor: a decided approval's status changed (must stay FALSE)

vars == <<apprStatus, horizon, opStatus, effect, clock>>
monVars == <<vars, lateEffect, rewritten>>

\* ------------------------------------------------------------------- actions --
\* park_operation: the operation reaches the gate and waits for a decision. The
\* caller's request carries the initial window.
Park(o, h) ==
  /\ opStatus[o] = "none" /\ apprStatus[o] = "none"
  /\ h \in 0..MaxTime
  /\ apprStatus' = [apprStatus EXCEPT ![o] = IF ParksWithoutARow THEN "none" ELSE "PENDING"]
  /\ horizon' = [horizon EXCEPT ![o] = h]
  /\ opStatus' = [opStatus EXCEPT ![o] = "PARKED"]
  /\ lateEffect' = lateEffect
  /\ rewritten' = rewritten
  /\ UNCHANGED <<effect, clock>>

\* change_approval(allow): a PENDING approval becomes APPROVED, and the decision
\* carries the window the user granted (the code updates `expires_at` here). An
\* approval whose *window* passed can still be approved — its status is PENDING —
\* which is why the dispatch check, not the decision, is what the horizon gates.
Approve(o, h) ==
  /\ apprStatus[o] = "PENDING"
  /\ h \in 0..MaxTime
  /\ apprStatus' = [apprStatus EXCEPT ![o] = "APPROVED"]
  /\ horizon' = [horizon EXCEPT ![o] = h]
  /\ lateEffect' = lateEffect
  /\ rewritten' = rewritten
  /\ UNCHANGED <<opStatus, effect, clock>>

\* change_approval(deny): the decision is no, and no effect may follow it.
Deny(o) ==
  /\ apprStatus[o] = "PENDING"
  /\ apprStatus' = [apprStatus EXCEPT ![o] = "DENIED"]
  /\ lateEffect' = lateEffect
  /\ rewritten' = rewritten
  /\ UNCHANGED <<horizon, opStatus, effect, clock>>

\* the clock moves; nothing else does (the poll pace and the wall clock are the
\* real machine's, and the window is measured against this one).
Tick ==
  /\ clock < MaxTime
  /\ clock' = clock + 1
  /\ lateEffect' = lateEffect
  /\ rewritten' = rewritten
  /\ UNCHANGED <<apprStatus, horizon, opStatus, effect>>

\* dispatch_operation: the linearization point. The effect lands only under an
\* APPROVED approval whose horizon is still ahead.
Dispatch(o) ==
  /\ apprStatus[o] = "APPROVED" /\ effect[o] = 0
  /\ (DropsExpiryCheck \/ horizon[o] = nohorizon \/ clock < horizon[o])
  /\ effect' = [effect EXCEPT ![o] = 1]
  /\ opStatus' = [opStatus EXCEPT ![o] = "CLOSED"]
  /\ lateEffect' = (lateEffect \/ (horizon[o] # nohorizon /\ clock >= horizon[o]))
  /\ rewritten' = rewritten
  /\ UNCHANGED <<apprStatus, horizon, clock>>

\* The operation closes without an effect (a cancellation, a settlement by
\* another path): `expire_pending_approvals` takes its pending approval with it.
Close(o) ==
  /\ opStatus[o] = "PARKED" /\ effect[o] = 0
  /\ opStatus' = [opStatus EXCEPT ![o] = "CLOSED"]
  /\ apprStatus' = [ x \in Ops |->
                       IF x = o /\ ~KeepsPendingOnClose /\ apprStatus[x] = "PENDING"
                       THEN "EXPIRED" ELSE apprStatus[x] ]
  /\ lateEffect' = lateEffect
  /\ rewritten' = rewritten
  /\ UNCHANGED <<horizon, effect, clock>>

\* Counterfactual: a decided approval can be rewritten (the daemon that treats a
\* decision as provisional). `rewritten` records it.
RewriteDecision(o, s) ==
  /\ RewritesDecisions
  /\ apprStatus[o] \in Decided /\ s \in Decided /\ s # apprStatus[o]
  /\ apprStatus' = [apprStatus EXCEPT ![o] = s]
  /\ rewritten' = TRUE
  /\ lateEffect' = lateEffect
  /\ UNCHANGED <<horizon, opStatus, effect, clock>>

Stutter == UNCHANGED monVars

Next ==
  \/ \E o \in Ops : \E h \in 0..MaxTime : Park(o, h)
  \/ \E o \in Ops : \E h \in 0..MaxTime : Approve(o, h)
  \/ \E o \in Ops : Deny(o)
  \/ Tick
  \/ \E o \in Ops : Dispatch(o)
  \/ \E o \in Ops : Close(o)
  \/ \E o \in Ops : \E s \in Decided : RewriteDecision(o, s)
  \/ Stutter

Init ==
  /\ apprStatus = [ o \in Ops |-> "none" ]
  /\ horizon = [ o \in Ops |-> nohorizon ]
  /\ opStatus = [ o \in Ops |-> "none" ]
  /\ effect = [ o \in Ops |-> 0 ]
  /\ clock = 0
  /\ lateEffect = FALSE
  /\ rewritten = FALSE

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A o \in Ops : apprStatus[o] \in ApprStatus
  /\ \A o \in Ops : horizon[o] \in 0..MaxTime \cup {nohorizon}
  /\ \A o \in Ops : opStatus[o] \in OpStatus
  /\ \A o \in Ops : effect[o] \in 0..1
  /\ clock \in 0..MaxTime

\* A08/§6.1: an effect exists only under an approval that was granted
EffectImpliesApproved ==
  \A o \in Ops : effect[o] = 1 => apprStatus[o] = "APPROVED"

\* The window is a real gate, not decoration: an effect never lands with the
\* horizon already reached (recorded over the transition, since "when" the
\* effect landed is not a state fact)
NoLateEffect == lateEffect = FALSE

\* A25/RT-06: a decision is final — APPROVED, DENIED and EXPIRED are never
\* rewritten (recorded the same way)
ApprovalDecisionIsFinal == rewritten = FALSE

\* A25/RT-06: an operation that closed leaves no pending approval behind, because
\* `expire_pending_approvals` runs in the same transaction
TerminalOperationHasNoPendingApproval ==
  \A o \in Ops : opStatus[o] = "CLOSED" => apprStatus[o] # "PENDING"

\* a parked operation waits behind an approval *row* — the thing the user decides
\* and the dispatch re-check reads — rather than on nothing. (It is not "pending
\* or approved": a denial leaves the operation parked with a decided row, which is
\* the state the user sees as "decision_open".)
ParkedHasAnApprovalRow ==
  \A o \in Ops : opStatus[o] = "PARKED" => apprStatus[o] # "none"

=============================================================================
