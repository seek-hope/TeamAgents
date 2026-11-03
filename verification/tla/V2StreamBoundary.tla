---------------------------- MODULE V2StreamBoundary -------------------------
EXTENDS Naturals, FiniteSets
(***************************************************************************)
(* Boundary abstraction for D-494 model-stream ingestion, not a byte-parser *)
(* refinement. Chunks may end before a full frame. Only a complete valid    *)
(* event may renew the stall deadline; only a complete response is selected.*)
(* A transport error may retry only before visible text, within the request*)
(* budget. EOF cannot turn buffered partial tool arguments into an effect. *)
(* Anchors: providers/stream.rs, finish_chat_message, Driver::step_model.    *)
(* UTF-8 parsing, HTTP/TLS, JSON semantics and provider truth stay external.*)
(***************************************************************************)
CONSTANT Bug, Limit, RetryLimit
VARIABLES
  s
vars == <<s>>
Init ==
  s = [phase |-> "reading", bytes |-> 0, partial |-> FALSE,
       visible |-> FALSE, full |-> FALSE, valid |-> TRUE,
       selected |-> FALSE, effect |-> FALSE, retries |-> 0,
       age |-> 0, elapsed |-> 0, renewals |-> 0, events |-> 0,
       retriedVisible |-> FALSE, deliveredInvalid |-> FALSE]
Read(chunk, event, valid, text, final) ==
  /\ s.phase = "reading" /\ s.elapsed < 4 /\ s.age < 2
  /\ chunk \in 1..2 /\ event \in BOOLEAN /\ valid \in BOOLEAN
  /\ text \in BOOLEAN /\ final \in BOOLEAN
  /\ (text \/ final) => event
  /\ IF s.bytes + chunk > Limit /\ Bug # "unbounded"
        THEN s' = [s EXCEPT !.phase = "failed"]
        ELSE IF event /\ ~valid /\ Bug # "invalid-event"
          THEN s' = [s EXCEPT !.phase = "failed", !.valid = FALSE]
          ELSE s' = [s EXCEPT !.bytes = @ + chunk, !.partial = ~event,
                       !.visible = @ \/ text, !.full = final,
                       !.valid = @ /\ (event => valid),
                       !.deliveredInvalid = @ \/ (event /\ ~valid),
                       !.phase = IF final THEN "complete" ELSE @,
                       !.events = @ + IF event THEN 1 ELSE 0,
                       !.renewals = @ + IF event \/ Bug = "partial-renews" THEN 1 ELSE 0,
                       !.age = IF event \/ Bug = "partial-renews" THEN 0 ELSE @]
Tick ==
  /\ s.phase = "reading" /\ s.elapsed < 4
  /\ s' = [s EXCEPT !.elapsed = @ + 1, !.age = @ + 1,
                   !.phase = IF s.age + 1 >= 2 THEN "failed" ELSE @]
Eof ==
  /\ s.phase = "reading"
  /\ s' = [s EXCEPT !.phase = IF Bug = "partial-complete" THEN "complete" ELSE "failed"]
Cancel ==
  /\ s.phase \in {"reading", "complete"}
  /\ s' = [s EXCEPT !.phase = "cancelled"]
Retry ==
  /\ s.phase = "failed" /\ ~s.selected
  /\ s.retries < RetryLimit \/ (Bug = "extra-retry" /\ s.retries = RetryLimit)
  /\ ~s.visible \/ Bug = "retry-visible"
  /\ s' = [s EXCEPT !.phase = "reading", !.bytes = 0, !.partial = FALSE,
                   !.full = FALSE, !.valid = TRUE, !.age = 0, !.elapsed = 0,
                   !.retriedVisible = @ \/ s.visible, !.retries = @ + 1]
Select ==
  /\ s.phase = "complete" /\ ~s.selected
  /\ s' = [s EXCEPT !.selected = TRUE]
Execute ==
  /\ s.selected /\ ~s.effect
  /\ s' = [s EXCEPT !.effect = TRUE]
Next == (\E chunk \in 1..2, event, valid, text, final \in BOOLEAN : Read(chunk, event, valid, text, final))
     \/ Tick \/ Eof \/ Cancel \/ Retry \/ Select \/ Execute \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ s.phase \in {"reading", "complete", "failed", "cancelled"}
  /\ s.bytes \in 0..(Limit + 2) /\ s.retries \in 0..(RetryLimit + 1)
  /\ s.age \in 0..2 /\ s.elapsed \in 0..4
BodyIsBounded == s.bytes <= Limit
OnlyCompleteResponsesExecute == s.selected \/ s.effect => s.full /\ s.valid
InvalidEventsAreRejected == ~s.deliveredInvalid
PartialChunksDoNotRenew == s.renewals <= s.events
VisibleTextIsNotReplayed == ~s.retriedVisible
RetryBudgetIsBounded == s.retries <= RetryLimit
=============================================================================
