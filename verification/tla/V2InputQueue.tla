-------------------------- MODULE V2InputQueue --------------------------
(* D-451: serialized input snapshot admission, revision and consumption.
   The transaction is an atomic step; filesystem publication, hashes and SQLite
   implementation are abstracted. Rust bridges: core/tests/input_queue.rs,
   engine/src/v2/attachments_tests.rs and engine/tests/cases/inputs.rs.
   Identity includes user/session/recipient/epoch. This is safety, not refinement. *)
EXTENDS Naturals, Sequences
CONSTANTS MutateSettled, IgnoreRevision, LoseOriginal, RereadSource, ReplayDelivery, TrustInvalid
VARIABLES
  state,
  revision,
  payload,
  history,
  source,
  snapshot,
  verified,
  delivered,
  deliveries,
  last
vars == <<state, revision, payload, history, source, snapshot, verified,
          delivered, deliveries, last>>
Record(op, ok) == [op |-> op, ok |-> ok]
Init ==
  /\ state = "empty"
  /\ revision = 0 /\ payload = 1 /\ history = <<>>
  /\ source = 1 /\ snapshot = 0 /\ verified = FALSE
  /\ delivered = 0 /\ deliveries = 0
  /\ last = Record("init", TRUE)
Capture(valid) ==
  /\ state = "empty" /\ snapshot = 0

  /\ snapshot' = source
  /\ verified' = valid

  /\ last' = Record("capture", TRUE)
  /\ UNCHANGED <<state, revision, payload, history, source, delivered, deliveries>>
Submit ==
  /\ state = "empty" /\ snapshot # 0
  /\ verified \/ TrustInvalid

  /\ state' = "pending"
  /\ history' = <<payload>>

  /\ last' = Record("submit", verified)
  /\ UNCHANGED <<revision, payload, source, snapshot, verified, delivered, deliveries>>
ChangeSource ==
  /\ source = 1
  /\ source' = 2

  /\ last' = Record("source", TRUE)
  /\ UNCHANGED <<state, revision, payload, history, snapshot, verified, delivered, deliveries>>
Revise(expected, validIdentity, cancel) ==
  /\ state # "empty" /\ revision < 2
  /\ state = "pending" \/ MutateSettled
  /\ expected = revision \/ IgnoreRevision
  /\ validIdentity

  /\ revision' = revision + 1

  /\ payload' = IF cancel THEN payload ELSE 3 - payload

  /\ history' = IF LoseOriginal THEN <<payload'>> ELSE Append(history, payload')

  /\ state' = IF cancel THEN "cancelled" ELSE state

  /\ last' = Record("revise", state = "pending" /\ expected = revision /\ validIdentity)
  /\ UNCHANGED <<source, snapshot, verified, delivered, deliveries>>
Consume ==
  /\ state = "pending" \/ (ReplayDelivery /\ state = "applied" /\ deliveries < 2)

  /\ state' = "applied"
  /\ deliveries' = deliveries + 1

  /\ delivered' = IF RereadSource THEN source ELSE snapshot

  /\ last' = Record("consume", TRUE)
  /\ UNCHANGED <<revision, payload, history, source, snapshot, verified>>
Retry ==
  /\ state # "empty"

  /\ last' = Record("retry", TRUE)
  /\ UNCHANGED <<state, revision, payload, history, source, snapshot, verified, delivered, deliveries>>
Next == (\E valid \in BOOLEAN : Capture(valid)) \/ Submit \/ ChangeSource
  \/ (\E expected \in 0..2, identity \in BOOLEAN, cancel \in BOOLEAN : Revise(expected, identity, cancel))
  \/ Consume \/ Retry \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------- invariants --
TypeOK == state \in {"empty", "pending", "applied", "cancelled"}
  /\ revision \in 0..2 /\ payload \in {1,2} /\ history \in Seq({1,2})
  /\ source \in {1,2} /\ snapshot \in 0..2 /\ verified \in BOOLEAN
  /\ delivered \in 0..2 /\ deliveries \in 0..2
VerifiedAdmission == state # "empty" => verified
PendingRevisionOnly == last.op = "revise" => last.ok
OriginalRetained == state # "empty" => history[1] = 1 /\ Len(history) = revision + 1
SnapshotDelivery == deliveries > 0 => delivered = snapshot
AtMostOnceDelivery == deliveries <= 1
=============================================================================
