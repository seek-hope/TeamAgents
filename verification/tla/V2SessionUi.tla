-------------------------- MODULE V2SessionUi --------------------------
(* D-446: session selection and confirmed deletion in the user client.
   The engine's successful receipt abstracts stop/lock/filesystem work;
   actual filesystem races, process identity and delivery are tested in Rust.
   Anchors: tui/src/sessions.rs, engine/src/v2/sessions.rs. *)
EXTENDS Naturals
CONSTANTS UnconfirmedDelete, EarlySwitch, EarlyExit, KeepDeletedOpen
VARIABLES current, selected, confirmed, pending, open, last
vars == <<current, selected, confirmed, pending, open, last>>
Sessions == {"a", "b"}
Init ==
  /\ current = "a" /\ selected = "a" /\ confirmed = "none"
  /\ pending = "none" /\ open = TRUE
  /\ last = [op |-> "none", target |-> "a", previous |-> "a", confirmed |-> FALSE, ok |-> FALSE]
Select(s) ==
  /\ open /\ pending = "none"
  /\ selected' = s /\ confirmed' = "none"
  /\ UNCHANGED <<current, pending, open, last>>
AskDelete ==
  /\ open /\ pending = "none"
  /\ confirmed' = selected
  /\ UNCHANGED <<current, selected, pending, open, last>>
Start(op) ==
  /\ open /\ pending = "none"
  /\ (op = "switch" \/ (op = "delete" /\ (confirmed = selected \/ UnconfirmedDelete)))
  /\ pending' = op
  /\ last' = [op |-> op, target |-> selected, previous |-> current,
               confirmed |-> confirmed = selected, ok |-> FALSE]
  /\ current' = IF op = "switch" /\ EarlySwitch THEN selected ELSE current
  /\ open' = IF op = "delete" /\ selected = current /\ EarlyExit THEN FALSE ELSE open
  /\ UNCHANGED <<selected, confirmed>>
Finish(ok) ==
  /\ pending # "none"
  /\ current' = IF pending = "switch" /\ ok THEN last.target ELSE current
  /\ open' = IF pending = "delete" /\ ok /\ last.target = current /\ ~KeepDeletedOpen
             THEN FALSE ELSE open
  /\ last' = [last EXCEPT !.ok = ok]
  /\ pending' = "none" /\ confirmed' = "none"
  /\ UNCHANGED selected
Next == (\E s \in Sessions : Select(s)) \/ AskDelete
        \/ (\E op \in {"switch", "delete"} : Start(op))
        \/ (\E ok \in BOOLEAN : Finish(ok)) \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------- invariants --
TypeOK == current \in Sessions /\ selected \in Sessions
          /\ confirmed \in Sessions \cup {"none"}
          /\ pending \in {"none", "switch", "delete"} /\ open \in BOOLEAN
ConfirmedDelete == last.op = "delete" => last.confirmed
SwitchAfterSuccess == last.op = "switch" /\ ~last.ok => current = last.previous
ExitAfterSuccess == ~open => last.op = "delete" /\ last.ok /\ last.target = current
DeletedCurrentExits == last.op = "delete" /\ last.ok /\ last.target = current => ~open
=============================================================================
