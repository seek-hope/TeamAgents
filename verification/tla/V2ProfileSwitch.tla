---------------------------- MODULE V2ProfileSwitch --------------------------
EXTENDS Naturals
(* set_instance_profile is user-only and limited to READY/WAITING. A request *)
(* captures its profile at admission; switching never rewrites that snapshot.*)
(* Model names stand for already resolved native profiles; adapter fields and*)
(* context-window values are checked by Rust/provider tests, not this model. *)
CONSTANT Bug
VARIABLES
  s
vars == <<s>>
Busy == {"MODEL_PENDING", "TOOLS_PENDING", "COMPLETION_PENDING"}
Init == s = [phase |-> "READY", model |-> "a", request |-> "none", captured |-> "none",
             changed |-> FALSE, actor |-> "none", safe |-> TRUE, terminated |-> FALSE]
Begin ==
  /\ s.phase = "READY" /\ ~s.terminated
  /\ s' = [s EXCEPT !.phase = "MODEL_PENDING", !.request = s.model, !.captured = s.model]
Advance(phase) ==
  /\ s.phase \in Busy /\ phase \in {"READY", "WAITING", "TOOLS_PENDING", "COMPLETION_PENDING"}
  /\ s' = [s EXCEPT !.phase = phase,
                   !.request = IF phase \in Busy THEN @ ELSE "none"]
Wake == /\ s.phase = "WAITING" /\ s' = [s EXCEPT !.phase = "READY"]
Switch(actor) ==
  /\ ~s.changed /\ actor \in {"user", "instance", "system"}
  /\ actor = "user" \/ Bug = "nonuser"
  /\ ~s.terminated \/ Bug = "terminated"
  /\ s.phase \notin Busy \/ Bug = "midturn"
  /\ s' = [s EXCEPT !.model = "b", !.changed = TRUE, !.actor = actor,
                   !.safe = s.phase \notin Busy /\ ~s.terminated,
                   !.request = IF Bug = "rewrite-request" /\ s.request # "none" THEN "b" ELSE @]
Rewrite ==
  /\ Bug = "rewrite-request" /\ s.request # "none"
  /\ s' = [s EXCEPT !.request = IF s.captured = "a" THEN "b" ELSE "a"]
Terminate == /\ ~s.terminated /\ s' = [s EXCEPT !.terminated = TRUE]
Next == Begin \/ (\E p \in {"READY", "WAITING", "TOOLS_PENDING", "COMPLETION_PENDING"} : Advance(p))
     \/ Wake \/ (\E a \in {"user", "instance", "system"} : Switch(a)) \/ Rewrite \/ Terminate
     \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK == s.model \in {"a", "b"} /\ s.request \in {"none", "a", "b"}
OnlyUserSwitches == s.changed => s.actor = "user"
SwitchUsesSafeBoundary == s.changed => s.safe
RequestProfileIsFrozen == s.request # "none" => s.request = s.captured
=============================================================================
