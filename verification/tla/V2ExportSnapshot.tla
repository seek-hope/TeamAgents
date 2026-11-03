----------------------------- MODULE V2ExportSnapshot ------------------------
EXTENDS Naturals, FiniteSets
(* export_limit: one read snapshot across pages, bounded temporary output, *)
(* flush/sync then no-replacement publication. SQL/filesystem atomicity are *)
(* assumptions. A crash after publication can leave a valid output despite *)
(* the caller receiving no success; this model does not erase that output. *)
(* Cancellation is sampled before hard_link. A later cancellation cannot  *)
(* undo publication; CheckPublication and Publish are separate transitions.*)
CONSTANT Bug, Limit
VARIABLES
  s
vars == <<s>>
Rows == {1, 2}
Init == s = [live |-> {1}, captured |-> {}, reading |-> FALSE, seen |-> {},
             bytes |-> 0, complete |-> FALSE, synced |-> FALSE, cancelled |-> FALSE,
             destination |-> "absent", published |-> FALSE, replaced |-> FALSE,
             publishSafe |-> FALSE, checked |-> FALSE]
Append == /\ s.live = {1} /\ s' = [s EXCEPT !.live = Rows]
Snapshot == /\ ~s.reading /\ s' = [s EXCEPT !.reading = TRUE, !.captured = s.live]
Page(row, size) ==
  /\ s.reading /\ ~s.complete /\ ~s.cancelled /\ size \in 1..2
  /\ row \in (IF Bug = "live-pages" THEN s.live ELSE s.captured) \ s.seen
  /\ s.bytes + size <= Limit \/ Bug = "unbounded"
  /\ s' = [s EXCEPT !.seen = @ \cup {row}, !.bytes = @ + size]
Complete ==
  /\ s.reading /\ ~s.complete /\ s.seen = s.captured /\ ~s.cancelled
  /\ s' = [s EXCEPT !.complete = TRUE]
Sync == /\ s.complete /\ ~s.synced /\ s' = [s EXCEPT !.synced = TRUE]
Cancel == /\ ~s.cancelled /\ s' = [s EXCEPT !.cancelled = TRUE]
Competitor == /\ s.destination = "absent" /\ s' = [s EXCEPT !.destination = "other"]
CheckPublication ==
  /\ s.complete /\ s.synced /\ ~s.cancelled /\ ~s.checked
  /\ s' = [s EXCEPT !.checked = TRUE]
Publish ==
  /\ ~s.published
  /\ s.checked \/ Bug = "early-publish"
  /\ s.destination = "absent" \/ Bug = "replace"
  /\ s' = [s EXCEPT !.destination = "ours", !.published = TRUE,
                   !.replaced = s.destination = "other",
                   !.publishSafe = s.complete /\ s.synced /\ s.checked]
Next == Append \/ Snapshot \/ (\E r \in Rows, size \in 1..2 : Page(r, size))
     \/ Complete \/ Sync \/ Cancel \/ Competitor \/ CheckPublication \/ Publish \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK == s.seen \subseteq Rows /\ s.captured \subseteq Rows /\ s.bytes \in 0..4
PagesShareSnapshot == s.seen \subseteq s.captured
ExportBytesAreBounded == s.bytes <= Limit
PublicationIsComplete == s.published => s.publishSafe /\ s.seen = s.captured
ExistingDestinationSurvives == ~s.replaced
=============================================================================
