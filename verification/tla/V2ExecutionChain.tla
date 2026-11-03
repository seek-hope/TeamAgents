--------------------------- MODULE V2ExecutionChain --------------------------
EXTENDS Naturals, FiniteSets
(***************************************************************************)
(* Composition slice: control dispatch -> pre_tool -> durable runner accept *)
(* -> external effect -> journal -> DB receipt -> owning epoch consumption. *)
(* Anchors: dispatch_operation, close_epoch_execution, Driver::step_tools,   *)
(* execute_shell, jobs::Runner::go. This is not a composition theorem for   *)
(* the other modules. One operation, two epochs and one revocation suffice *)
(* to expose stale approval, replay and late-receipt crossings. Filesystem *)
(* writes and signals remain abstract. A cancelled assignment is not a     *)
(* proof of stopped execution; a stopped observation never edits a receipt.*)
(***************************************************************************)
CONSTANT Bug
VARIABLES
  s
vars == <<s>>
Terminal == {"SUCCEEDED", "FAILED", "UNKNOWN", "CANCELLED"}
Init ==
  s = [phase |-> "PREPARED", epoch |-> 0, opEpoch |-> 0,
       revision |-> 0, stamp |-> 0, grant |-> TRUE, active |-> TRUE,
       approval |-> "none", approvalRevision |-> 0, expired |-> FALSE,
       hook |-> "none", committed |-> FALSE, accepted |-> FALSE, effects |-> 0,
       journal |-> "none", receipt |-> "none", imported |-> FALSE,
       cancel |-> FALSE, stopped |-> FALSE, takeover |-> FALSE,
       dispatchValid |-> FALSE, crashed |-> FALSE, groupLive |-> FALSE, recoveredJournal |-> FALSE]
Approve ==
  /\ s.phase = "PREPARED" /\ s.approval = "none"
  /\ s' = [s EXCEPT !.approval = "approved", !.approvalRevision = s.revision]
Expire == /\ ~s.expired /\ s' = [s EXCEPT !.expired = TRUE]
Revoke ==
  /\ s.revision = 0
  /\ s' = [s EXCEPT !.revision = 1, !.grant = FALSE]
Pause == /\ s.active /\ s' = [s EXCEPT !.active = FALSE]
DispatchValid ==
  s.grant /\ s.active /\ s.stamp = s.revision /\ s.approval = "approved"
    /\ s.approvalRevision = s.revision /\ ~s.expired /\ s.epoch = s.opEpoch
Dispatch ==
  /\ s.phase = "PREPARED" /\ ~s.crashed
  /\ DispatchValid \/ Bug = "stale-dispatch"
  /\ s' = [s EXCEPT !.phase = "DISPATCH_COMMITTED", !.committed = TRUE, !.dispatchValid = DispatchValid]
Hook(outcome) ==
  /\ s.phase = "DISPATCH_COMMITTED" /\ s.hook = "none"
  /\ outcome \in {"allow", "deny"} /\ ~s.crashed
  /\ s' = [s EXCEPT !.hook = outcome,
                   !.phase = IF outcome = "deny" THEN "FAILED" ELSE @,
                   !.receipt = IF outcome = "deny" THEN "FAILED" ELSE @]
Accept ==
  /\ s.phase = "DISPATCH_COMMITTED" /\ s.hook = "allow" /\ ~s.accepted /\ ~s.crashed
  /\ s' = [s EXCEPT !.accepted = TRUE]
EarlyAccept ==
  /\ Bug = "accept-before-commit" /\ s.phase = "PREPARED" /\ ~s.accepted
  /\ s' = [s EXCEPT !.accepted = TRUE, !.hook = "allow"]
Effect ==
  \* The runner is independent of coordinator crashes and DB recovery. A GO
  \* already accepted may start after UNKNOWN; that is not a fresh dispatch.
  /\ ~s.stopped /\ s.hook = "allow"
  /\ s.accepted \/ Bug = "effect-before-accept"
  /\ s.effects = 0
  /\ s' = [s EXCEPT !.phase = IF s.receipt \in Terminal THEN @ ELSE "RUNNING",
                   !.effects = @ + 1, !.groupLive = TRUE]
Finish(result, remains) ==
  /\ s.effects = 1 /\ s.journal = "none"
  /\ result \in {"SUCCEEDED", "FAILED", "CANCELLED"} /\ remains \in BOOLEAN
  /\ s.stopped => ~remains
  \* The runner can finish after DB recovery recorded UNKNOWN. Its journal
  \* stays separate; a later observed stop does not rewrite that DB receipt.
  /\ s' = [s EXCEPT !.phase = IF s.receipt \in Terminal THEN @ ELSE result,
                   !.journal = result, !.groupLive = remains]
Crash ==
  /\ ~s.crashed
  /\ s' = [s EXCEPT !.crashed = TRUE]
Recover ==
  /\ s.crashed
  /\ s' = [s EXCEPT !.crashed = FALSE,
                   !.recoveredJournal = @ \/ (s.journal # "none" /\ s.receipt = "none"),
                   !.receipt = IF s.receipt \in Terminal THEN @
                                ELSE IF s.journal # "none" THEN s.journal
                                ELSE IF s.accepted THEN "UNKNOWN" ELSE @,
                   !.phase = IF s.receipt \in Terminal THEN @
                              ELSE IF s.journal # "none" THEN s.journal
                              ELSE IF s.accepted THEN "UNKNOWN" ELSE @]
Persist ==
  /\ s.journal # "none" /\ s.receipt = "none" /\ ~s.crashed
  /\ s' = [s EXCEPT !.receipt = s.journal]
Cancel ==
  /\ ~s.cancel
  /\ s' = [s EXCEPT !.cancel = TRUE,
                   !.phase = IF s.phase = "PREPARED" THEN "CANCELLED" ELSE @,
                   !.receipt = IF s.phase = "PREPARED" THEN "CANCELLED" ELSE @,
                   !.stopped = IF s.phase = "PREPARED" THEN TRUE ELSE @]
Reset ==
  /\ s.epoch = 0
  /\ s' = [s EXCEPT !.epoch = 1, !.imported = FALSE, !.cancel = TRUE,
                   !.phase = IF s.phase = "PREPARED" THEN "CANCELLED" ELSE @,
                   !.receipt = IF s.phase = "PREPARED" THEN "CANCELLED" ELSE @,
                   !.stopped = IF s.phase = "PREPARED" THEN TRUE ELSE @]
Import ==
  /\ s.receipt \in Terminal /\ ~s.imported
  /\ s.epoch = s.opEpoch \/ Bug = "import-new-epoch"
  /\ s' = [s EXCEPT !.imported = TRUE]
GroupStops ==
  /\ s.cancel /\ s.groupLive
  /\ s' = [s EXCEPT !.groupLive = FALSE]
ObserveStop ==
  /\ s.cancel /\ ~s.stopped /\ ~s.groupLive /\ s.phase \in Terminal /\ s.effects > 0
  \* The external observer proved absence of the recorded group. It does not
  \* establish absence of escaped descendants or remote effects.
  /\ s' = [s EXCEPT !.stopped = TRUE,
                   !.receipt = IF Bug = "rewrite-unknown" /\ s.receipt = "UNKNOWN"
                                THEN "CANCELLED" ELSE @]
Takeover ==
  /\ s.cancel /\ ~s.takeover
  /\ s.stopped \/ Bug = "cancel-is-stop"
  /\ s' = [s EXCEPT !.takeover = TRUE]
ReplayUnknown ==
  /\ Bug = "replay-unknown" /\ s.receipt = "UNKNOWN" /\ s.effects = 1
  /\ s' = [s EXCEPT !.effects = 2]
VetoEffect ==
  /\ Bug = "ignore-veto" /\ s.hook = "deny" /\ s.effects = 0
  /\ s' = [s EXCEPT !.effects = 1, !.accepted = TRUE, !.groupLive = TRUE]
Next == Approve \/ Expire \/ Revoke \/ Pause \/ Dispatch \/ Accept \/ EarlyAccept \/ Effect
     \/ (\E outcome \in {"allow", "deny"} : Hook(outcome))
     \/ (\E result \in {"SUCCEEDED", "FAILED", "CANCELLED"}, remains \in BOOLEAN : Finish(result, remains))
     \/ Crash \/ Recover \/ Persist \/ Cancel \/ Reset \/ Import
     \/ GroupStops \/ ObserveStop \/ Takeover \/ ReplayUnknown \/ VetoEffect
Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ s.phase \in {"PREPARED", "DISPATCH_COMMITTED", "RUNNING"} \cup Terminal
  /\ s.receipt \in Terminal \cup {"none"}
  /\ s.effects \in 0..2 /\ s.epoch \in 0..1 /\ s.revision \in 0..1
DispatchWasAuthorized == s.phase = "DISPATCH_COMMITTED" => s.dispatchValid
EffectHasControlCommit == s.effects > 0 => s.committed
EffectHasDurableAcceptance == s.effects > 0 => s.accepted
VetoPreventsEffect == s.hook = "deny" => s.effects = 0
UnknownIsNeverReplayed == s.effects <= 1
ContextKeepsItsEpoch == s.imported => s.epoch = s.opEpoch
TakeoverNeedsStopEvidence == s.takeover => s.stopped /\ ~s.groupLive

\* --------------------------------------------------------------- properties --
TerminalReceiptIsImmutable ==
  [][s.receipt \in Terminal => s'.receipt = s.receipt]_vars
=============================================================================
