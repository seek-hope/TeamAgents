-------------------------- MODULE V2Presentation --------------------------
(* D-450: completion effects and viewport media admission.
   Anchors: tui/src/completion.rs and tui/src/graphics.rs (Document.resolve,
   render_request and poll); bridges: tui/tests/parity.rs and PTY smoke.
   Cache entries include reservations for current-generation pending results.
   The model abstracts layout, parser work, channels and image identity. It is
   safety only: eventual worker completion and rendering time are not proved. *)
EXTENDS Naturals, FiniteSets
CONSTANTS CompletionDispatch, CancelErases, EagerRender, CacheOverflow, ApplyStale
VARIABLES
  focus,
  draft,
  visible,
  queued,
  cached,
  generation,
  last
vars == <<focus, draft, visible, queued, cached, generation, last>>
Ids == {1, 2, 3}
Capacity == 2
CurrentQueued == {r.id : r \in {q \in queued : q.epoch = generation}}
Record(op, effect, ok) == [op |-> op, effect |-> effect, ok |-> ok]
Init ==
  /\ focus = "composer" /\ draft = "prefix"
  /\ visible = {1} /\ queued = {} /\ cached = {} /\ generation = 0
  /\ last = Record("init", "none", TRUE)
Edit(d) ==
  /\ draft' = d /\ last' = Record("edit", "none", TRUE)
  /\ UNCHANGED <<focus, visible, queued, cached, generation>>
Focus(f) ==
  /\ focus' = f /\ last' = Record("focus", "none", TRUE)
  /\ UNCHANGED <<draft, visible, queued, cached, generation>>
Select ==
  /\ focus = "composer" /\ draft = "prefix"
  /\ last' = Record("select", "none", TRUE)
  /\ UNCHANGED <<focus, draft, visible, queued, cached, generation>>
Fill ==
  /\ focus = "composer" /\ draft = "prefix"
  /\ draft' = "filled"
  /\ last' = Record("fill", IF CompletionDispatch THEN "command" ELSE "none", TRUE)
  /\ UNCHANGED <<focus, visible, queued, cached, generation>>
Cancel ==
  /\ focus = "composer" /\ draft = "prefix"
  /\ draft' = IF CancelErases THEN "empty" ELSE draft
  /\ last' = Record("cancel", "none", draft' = draft)
  /\ UNCHANGED <<focus, visible, queued, cached, generation>>
Submit ==
  /\ focus = "composer" /\ draft = "filled"
  /\ draft' = "empty" /\ last' = Record("submit", "command", TRUE)
  /\ UNCHANGED <<focus, visible, queued, cached, generation>>
Scroll(v) ==
  /\ visible' = v /\ last' = Record("scroll", "none", TRUE)
  /\ UNCHANGED <<focus, draft, queued, cached, generation>>
Enqueue(id) ==
  /\ id \notin cached \cup CurrentQueued
  /\ id \in visible \/ EagerRender
  /\ Cardinality(queued) < Capacity
  /\ Cardinality(cached \cup CurrentQueued) < Capacity \/ CacheOverflow
  /\ queued' = queued \cup {[id |-> id, epoch |-> generation]}
  /\ last' = Record("enqueue", "none", id \in visible)
  /\ UNCHANGED <<focus, draft, visible, cached, generation>>
Complete(r) ==
  /\ r \in queued
  /\ queued' = queued \ {r}
  /\ cached' = IF r.epoch = generation \/ ApplyStale THEN cached \cup {r.id} ELSE cached
  /\ last' = Record("complete", "none", r.epoch = generation \/ cached' = cached)
  /\ UNCHANGED <<focus, draft, visible, generation>>
Evict(id) ==
  /\ id \in cached /\ id \notin visible
  /\ cached' = cached \ {id} /\ last' = Record("evict", "none", TRUE)
  /\ UNCHANGED <<focus, draft, visible, queued, generation>>
Reload ==
  /\ generation = 0 /\ generation' = 1 /\ cached' = {}
  /\ last' = Record("reload", "none", TRUE)
  /\ UNCHANGED <<focus, draft, visible, queued>>
Next == (\E d \in {"prefix", "plain", "filled"} : Edit(d))
  \/ (\E f \in {"composer", "approvals"} : Focus(f))
  \/ Select \/ Fill \/ Cancel \/ Submit
  \/ (\E v \in SUBSET Ids : Scroll(v))
  \/ (\E id \in Ids : Enqueue(id) \/ Evict(id))
  \/ (\E r \in queued : Complete(r)) \/ Reload \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------- invariants --
TypeOK == focus \in {"composer", "approvals"} /\ draft \in {"prefix", "plain", "filled", "empty"}
  /\ visible \subseteq Ids /\ cached \subseteq Ids /\ generation \in {0, 1}
  /\ queued \subseteq [id: Ids, epoch: {0, 1}]
CompletionOnlyEdits == last.op \in {"fill", "select", "cancel"} => last.effect = "none"
CancelKeepsDraft == last.op = "cancel" => last.ok
OnlyVisibleAdmitted == last.op = "enqueue" => last.ok
CacheIsBounded == Cardinality(cached \cup CurrentQueued) <= Capacity /\ Cardinality(queued) <= Capacity
StaleResultIgnored == last.op = "complete" => last.ok
=============================================================================
