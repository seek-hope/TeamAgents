------------------------------ MODULE V2Concurrency ---------------------------
(***************************************************************************)
(* The daemon's concurrent readers beside its single writer (§9/§4.1, A28).   *)
(*                                                                           *)
(* The daemon holds the session's log behind one write connection while      *)
(* `checkpoint`, `events` and `history` reads run on their own connections    *)
(* (§4.1: mid-turn reads never contend for the single writer), and A28 states *)
(* what that structure buys: a slow client cannot block the writer, and a     *)
(* reader that follows its cursor never loses or reorders a committed event.  *)
(* D-249's daemon module covers the writer's own log — the `events(since)`    *)
(* handover is the full range and never a gap — and this module adds the      *)
(* interleavings *across* connections, which the single-writer control plane  *)
(* could not: reader steps are not fair (a stalled client is one that never   *)
(* runs) while the writer commits whatever its clients are doing.             *)
(*                                                                           *)
(* State: the writer's committed log is the sequence `1..len` (an event's id  *)
(* is its own position, so "lost" and "reordered" are decidable by comparing  *)
(* what a reader was handed with the prefix the writer committed); every      *)
(* reader owns a cursor `count[r]` and the view `view[r]` it was handed — the *)
(* id of its i-th delivered event. The environment (which reader runs, and    *)
(* when) is non-deterministic; that is exactly what is enumerated.            *)
(*                                                                           *)
(* The three claims:                                                          *)
(*                                                                           *)
(*   * **a slow or stalled reader never blocks the writer**                  *)
(*     (`WriterProgressesDespiteAStalledReader`, under weak fairness of the   *)
(*     writer's own step): the commit's guard names no reader;               *)
(*   * **a reader that follows its cursor sees exactly the committed log**    *)
(*     (`ReadersSeeTheCommittedPrefix`): the i-th event it was handed is the  *)
(*     i-th event the writer committed, so nothing on the way was lost or     *)
(*     reordered;                                                            *)
(*   * **and it never sees past the writer** (`NoReaderSeesUncommitted`): a   *)
(*     reader's cursor cannot run ahead of the committed log.                *)
(*                                                                           *)
(* Code anchors: engine/src/v2/daemon.rs (each connection is served by its own *)
(* task, so a client that reads slowly holds no lock on the log),             *)
(* engine/src/v2/storage.rs (one writer connection; `Storage::open` gives a   *)
(* reader its own), core/src/v2/control.rs (the `events`/`history` reads) and *)
(* core/src/kernel/instance.rs (the view a request is built from).            *)
(*                                                                           *)
(* What the model is and is not: it abstracts a client into a *cursor plus    *)
(* order*, not into pages, sockets or buffers — the bounded page and the      *)
(* `HandoversAreTheFullRange` rule are `V2Daemon`'s (D-249), and how much a   *)
(* stalled client may hold is `V2Inbox`'s bound, not this module's. It does   *)
(* not model the daemon's accept loop, bytes on the wire, or the storage      *)
(* worker's queue: the claim it checks is the ordering one A28 makes.         *)
(*                                                                           *)
(* The three counterfactuals are the plausible mistakes those claims exist    *)
(* against: `WriterWaitsForReaders` (a commit that waits for every client to  *)
(* catch up — a per-client queue a slow client fills), `ReaderMaySkip` (a     *)
(* read that hands over a committed event that is not the cursor's next one)  *)
(* and `ReaderMayRunAhead` (a read that hands over an event the writer has    *)
(* not committed).                                                           *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Readers,               \* the clients holding a read connection
          MaxEvents,             \* one writer's committed log is 1..MaxEvents
          WriterWaitsForReaders, \* counterfactual: the commit waits for every reader to catch up
          ReaderMaySkip,         \* counterfactual: a read hands over a committed event that is not the next one
          ReaderMayRunAhead      \* counterfactual: a read hands over an event the writer has not committed

Events == 1..MaxEvents

VARIABLES
  len,     \* the writer's committed log: events 1..len
  count,   \* per reader: how many events it has been handed
  view     \* per reader: the id of its i-th handed event, for i in Events

vars == <<len, count, view>>

\* ------------------------------------------------------------------ actions --
\* The writer commits the next event. The guard names no reader: that is the
\* structure A28 rests on, and the counterfactual's guard is where it would go.
WriterStep ==
  IF WriterWaitsForReaders
  THEN /\ len < MaxEvents
       /\ \A r \in Readers : count[r] = len
       /\ len' = len + 1
       /\ UNCHANGED <<count, view>>
  ELSE /\ len < MaxEvents
       /\ len' = len + 1
       /\ UNCHANGED <<count, view>>

\* A reader hands over the next committed event, in order — or it simply never
\* runs (reader steps are not fair: that is the slow client).
Read(r) ==
  /\ count[r] < len
  /\ view' = [view EXCEPT ![r][count[r] + 1] = count[r] + 1]
  /\ count' = [count EXCEPT ![r] = count[r] + 1]
  /\ UNCHANGED len

\* Counterfactual: the read hands over some other committed event — a page
\* assembled from the wrong end, or a listing that raced the writer.
ReadOutOfOrder(r) ==
  /\ ReaderMaySkip
  /\ count[r] < len
  /\ \E e \in (count[r] + 1)..len :
       /\ e # count[r] + 1
       /\ view' = [view EXCEPT ![r][count[r] + 1] = e]
       /\ count' = [count EXCEPT ![r] = count[r] + 1]
       /\ UNCHANGED len

\* Counterfactual: the read hands over an event the writer has not committed.
ReadAhead(r) ==
  /\ ReaderMayRunAhead
  /\ count[r] = len
  /\ len < MaxEvents
  /\ view' = [view EXCEPT ![r][count[r] + 1] = len + 1]
  /\ count' = [count EXCEPT ![r] = count[r] + 1]
  /\ UNCHANGED len

ReaderStep(r) ==
  \/ Read(r)
  \/ ReadOutOfOrder(r)
  \/ ReadAhead(r)

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED vars

Next ==
  \/ WriterStep
  \/ (\E r \in Readers : ReaderStep(r))
  \/ Stutter

Init ==
  /\ len = 0
  /\ count = [r \in Readers |-> 0]
  /\ view = [r \in Readers |-> [i \in Events |-> 0]]

\* Weak fairness only on the writer's own step: the daemon keeps committing while
\* there is room, whatever its clients are doing. Nothing is assumed about a
\* reader — a stalled client is not fair — which is what makes the claim below
\* say something rather than describe the guard.
Spec == Init /\ [][Next]_vars /\ WF_vars(WriterStep)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ len \in 0..MaxEvents
  /\ count \in [Readers -> 0..MaxEvents]
  /\ view \in [Readers -> [Events -> 0..MaxEvents]]

\* A28: a reader that follows its cursor was handed the committed log itself —
\* the i-th event it received is the i-th event the writer committed, so nothing
\* on the way was lost or reordered. (The control hands over a different
\* committed event and must be refuted.)
ReadersSeeTheCommittedPrefix ==
  \A r \in Readers : \A i \in Events : i <= count[r] => view[r][i] = i

\* A28's other half of "follows its cursor": a reader never sees an event the
\* writer has not committed. (The control hands one over and must be refuted.)
NoReaderSeesUncommitted == \A r \in Readers : count[r] <= len

\* --------------------------------------------------------------- properties --
\* A28: a slow or stalled reader never blocks a writer's commit. The writer's
\* step is the only one that commits, it is weak-fair, and its guard names no
\* reader — so the log fills however far behind a reader stays. The control makes
\* that guard wait for every reader, and a reader that never runs then starves
\* the writer: the state the claim forbids.
WriterProgressesDespiteAStalledReader == (len < MaxEvents) ~> (len = MaxEvents)

=============================================================================
