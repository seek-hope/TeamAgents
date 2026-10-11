-------------------------- MODULE V2DisplayWorker --------------------------
(* D-451: finite abstraction of one owned presentation process and bounded queue.
   Anchors: tui/src/graphics.rs and tui/src/subprocess.rs; bridges:
   tui/src/graphics_worker_tests.rs and subprocess.rs tests.
   A Poll step represents the parent observing cancellation/deadline. Stop/reap is
   atomic here. OS scheduling, filesystem and kernel liveness, syscall filter
   correctness and parser termination are NOT proved by this safety model. *)
EXTENDS Naturals, FiniteSets
CONSTANTS StartStale, KeepExpired, LeakProcess, ApplyStale, OverflowOutput
VARIABLES
  owner,
  generation,
  queued,
  active,
  live,
  epoch,
  age,
  bytes,
  cache,
  last
vars == <<owner, generation, queued, active, live, epoch, age, bytes, cache, last>>
Requests == [id: {1,2}, epoch: {0,1}]
Record(op, ok) == [op |-> op, ok |-> ok]
Init ==
  /\ owner = TRUE /\ generation = 0 /\ queued = {}
  /\ active = FALSE /\ live = FALSE /\ epoch = 0
  /\ age = 0 /\ bytes = 0 /\ cache = {}
  /\ last = Record("init", TRUE)
Enqueue(id) ==
  /\ owner /\ Cardinality(queued) < 2

  /\ queued' = queued \cup {[id |-> id, epoch |-> generation]}

  /\ last' = Record("enqueue", TRUE)
  /\ UNCHANGED <<owner, generation, active, live, epoch, age, bytes, cache>>
Start(r) ==
  /\ owner /\ ~active /\ r \in queued
  /\ r.epoch = generation \/ StartStale

  /\ queued' = queued \ {r}

  /\ active' = TRUE
  /\ live' = TRUE
  /\ epoch' = r.epoch

  /\ age' = 0
  /\ bytes' = 0

  /\ last' = Record("start", r.epoch = generation)
  /\ UNCHANGED <<owner, generation, cache>>
Skip(r) ==
  /\ r \in queued /\ r.epoch # generation

  /\ queued' = queued \ {r}

  /\ last' = Record("skip", TRUE)
  /\ UNCHANGED <<owner, generation, active, live, epoch, age, bytes, cache>>
Tick ==
  /\ active /\ age < 2

  /\ age' = age + 1
  /\ last' = Record("tick", TRUE)
  /\ UNCHANGED <<owner, generation, queued, active, live, epoch, bytes, cache>>
ReadOutput ==
  /\ active /\ (bytes < 2 \/ (OverflowOutput /\ bytes = 2))

  /\ bytes' = bytes + 1
  /\ last' = Record("read", TRUE)
  /\ UNCHANGED <<owner, generation, queued, active, live, epoch, age, cache>>
Poll ==
  /\ active /\ (age = 2 \/ epoch # generation)

  /\ active' = KeepExpired
  /\ live' = (KeepExpired \/ LeakProcess)

  /\ last' = Record("stop", ~active' /\ ~live')
  /\ UNCHANGED <<owner, generation, queued, epoch, age, bytes, cache>>
Finish ==
  /\ active

  /\ active' = FALSE
  /\ live' = LeakProcess

  /\ cache' = IF epoch = generation \/ ApplyStale THEN cache \cup {epoch} ELSE cache

  /\ last' = Record("finish", TRUE)
  /\ UNCHANGED <<owner, generation, queued, epoch, age, bytes>>
Reload ==
  /\ owner /\ generation = 0

  /\ generation' = 1
  /\ cache' = {}

  /\ last' = Record("reload", TRUE)
  /\ UNCHANGED <<owner, queued, active, live, epoch, age, bytes>>
Drop ==
  /\ owner
  /\ owner' = FALSE
  /\ queued' = {}

  /\ active' = FALSE
  /\ live' = LeakProcess
  /\ cache' = {}

  /\ last' = Record("drop", TRUE)
  /\ UNCHANGED <<generation, epoch, age, bytes>>
Next == (\E id \in {1,2} : Enqueue(id))
  \/ (\E r \in Requests : Start(r) \/ Skip(r))
  \/ Tick \/ ReadOutput \/ Poll \/ Finish \/ Reload \/ Drop \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------- invariants --
TypeOK == owner \in BOOLEAN /\ generation \in {0,1} /\ queued \subseteq Requests
  /\ active \in BOOLEAN /\ live \in BOOLEAN /\ epoch \in {0,1}
  /\ age \in 0..2 /\ bytes \in 0..3 /\ cache \subseteq {0,1}
CurrentStartOnly == last.op = "start" => last.ok
ObservedStopReaps == last.op = "stop" => last.ok
NoOrphanProcess == ~active => ~live
WorkerOutputBound == bytes <= 2 /\ Cardinality(queued) <= 2
CurrentResultOnly == cache \subseteq {generation}
OwnerDropReaps == ~owner => ~active /\ ~live /\ queued = {}
=============================================================================
