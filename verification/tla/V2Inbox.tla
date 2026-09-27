------------------------------ MODULE V2Inbox -------------------------------
(***************************************************************************)
(* The inbox and the application of an envelope (§5.3, A06, A24;             *)
(* D-63/D-72). A message is reported accepted only once it is persisted, and  *)
(* the recipient applies it exactly once at a safe boundary. The rules the    *)
(* code and the documents state, and what this model checks:                  *)
(*                                                                           *)
(*   * exactly once per envelope id — even when a crash loses the 'APPLIED'   *)
(*     marker that would have skipped the envelope: the context append        *)
(*     carries the id as its dedup key, so a replay appends nothing            *)
(*     (`AtMostOncePerEnvelope`; the one-case version is the test             *)
(*     `submit_input_applies_context_once_per_envelope`);                     *)
(*   * applications follow the envelope sequence (`LogIsIncreasing`, the      *)
(*     code's `ORDER BY sequence`);                                          *)
(*   * nothing accepted is ever silently dropped — it is applied or sealed    *)
(*     (`NoSilentLoss`: DESIGN's "apply backpressure instead of dropping      *)
(*     silently", the reason a full inbox fails the send openly);             *)
(*   * a stale-epoch envelope never reaches the new epoch's context           *)
(*     (`NoStaleApplication`, A24's "never injected into a new instance's     *)
(*     context": it is sealed SUPERSEDED instead);                           *)
(*   * only the user or the instance itself drains an inbox                   *)
(*     (`DrainIsOwned`, the same family as §5.1's "the user manages and       *)
(*     reads every instance");                                               *)
(*   * no send is accepted into a full inbox (`BoundedInbox` — the bound is   *)
(*     the *decision* at insert, which is what `queue_envelope` checks; a      *)
(*     lost marker may legitimately leave more rows pending than the cap).     *)
(*                                                                           *)
(* Code anchors: core/src/v2/control.rs — `queue_envelope` (the envelope      *)
(* persists as ACCEPTED inside the caller's transaction, with a `sequence`),  *)
(* `DEFAULT_MAX_INBOX` and the explicit "recipient … inbox full …:            *)
(* backpressure" send error, and `drain_inbox`: the identity check ("only the *)
(* user or the instance itself may drain its inbox"), the `WHERE state =      *)
(* 'ACCEPTED' ORDER BY sequence` read, the stale-epoch branch that seals      *)
(* SUPERSEDED and names the ids (`sealed_ids`, D-72), the `append_context(…)  *)
(* Some(&id) …)` whose id is the dedup key, and the 'APPLIED' marker beside   *)
(* it. The live half is `review/dogfood/crash.py` and `unknown_outcome.py`    *)
(* for the restart shapes and `queued_input.py` for D-72.                    *)
(*                                                                           *)
(* The ids *are* the envelope sequence numbers (`queue_envelope` assigns        *)
(* `MAX(sequence) + 1`), so the model makes arrivals in increasing id order     *)
(* part of what an id means; `LogIsIncreasing` then says the *drain* follows    *)
(* that order, which is the code's `ORDER BY sequence`.                        *)
(*                                                                           *)
(* What the model is and is not: one recipient's inbox, three envelopes and   *)
(* two epochs — enough for the rules above and no more — and one application  *)
(* step per envelope, so "at a safe boundary" is a step here while the        *)
(* driver's boundary machinery stays with `V2Control`                         *)
(* (`InputLandsAtTheBoundary`, `QueuedInputEntersTheContext`) and the tests.  *)
(* Senders are not modelled (an envelope is in the inbox or it is not), and   *)
(* neither is the `kind` vocabulary: every kind obeys the same bound and      *)
(* no-loss rule. DESIGN's *permission* for status notifications to coalesce   *)
(* has no implementation today (`coalesce` appears in no source file), so the *)
(* model does not carry it. It is safety only: whether a drain ever runs is   *)
(* the client's business, which is why there is no fairness assumption.       *)
(*                                                                           *)
(* `CrashLosesMarker` is the window the committed code closes with one        *)
(* transaction — the append and the marker are written together. The model    *)
(* keeps the window because the *dedup* is the guard the exactly-once         *)
(* argument rests on, and a caller's transaction discipline should not be the *)
(* only thing between a replay and a second application;                    *)
(* `MC_inbox_no_dedup.cfg` is the control that shows the difference.         *)
(*                                                                           *)
(* The five counterfactuals are the defects the rules exist against:          *)
(* `ApplyWithoutDedup` (an append that ignores the id it already applied),    *)
(* `DropWhenFull` (a full inbox discarding what is queued), `OverflowInbox`   *)
(* (no bound at all), `ApplySealed` (a stale-epoch envelope applied into the  *)
(* new epoch) and `AnyInstanceDrains` (a third party draining an inbox).      *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS MaxInbox,            \* the recipient's capacity (DEFAULT_MAX_INBOX in the code)
          ApplyWithoutDedup,   \* counterfactual: the append ignores the id it already applied
          DropWhenFull,        \* counterfactual: a full inbox discards what is queued
          OverflowInbox,       \* counterfactual: the inbox has no bound at all
          ApplySealed,         \* counterfactual: a stale-epoch envelope is applied, not sealed
          AnyInstanceDrains    \* counterfactual: an instance may drain another's inbox

ASSUME MaxInbox >= 1

Ids == 1..3                    \* envelope ids, in sequence order
Epochs == {0, 1}
States == {"free", "accepted", "applied", "superseded"}
Actors == {"user", "own", "other"}

VARIABLES
  state,          \* per id: free | accepted | applied | superseded
  birth,          \* per id: the epoch the envelope was accepted in
  epoch,          \* the instance's current epoch
  log,            \* the applied envelope ids, in application order (the context)
  everAccepted,   \* monitor: the ids that were accepted at some point
  appliedStale,   \* monitor: an envelope of an older epoch reached the context
  drainedByOther, \* monitor: an inbox was drained by a third party
  acceptedOverCap \* monitor: a send was accepted while the inbox was already full

vars == <<state, birth, epoch, log>>
monVars == <<vars, everAccepted, appliedStale, drainedByOther, acceptedOverCap>>

Accepted == {i \in Ids : state[i] = "accepted"}
ArrivedBelow(i) == \A j \in Ids : j < i => state[j] # "free"   \* ids are sequence numbers: they arrive in order
LogIds == {log[k] : k \in 1..Len(log)}
LowestPending == IF Accepted = {} THEN 0 ELSE CHOOSE i \in Accepted : \A j \in Accepted : i <= j

\* ------------------------------------------------------------------- actions --
\* The sender queues one envelope: it is persisted as accepted, so a report of
\* acceptance is a report about the log (§5.3) — and the bound is the inbox's
\* capacity, so a full inbox cannot grow.
Queue(i) ==
  /\ state[i] = "free"
  /\ ArrivedBelow(i)
  /\ Cardinality(Accepted) < MaxInbox
  /\ state' = [state EXCEPT ![i] = "accepted"]
  /\ birth' = [birth EXCEPT ![i] = epoch]
  /\ everAccepted' = everAccepted \cup {i}
  /\ UNCHANGED <<epoch, log, appliedStale, drainedByOther, acceptedOverCap>>

\* The inbox is full: the send fails openly so the sender can retry later, and
\* nothing is stored or dropped.
RefuseSend(i) ==
  /\ state[i] = "free"
  /\ ~DropWhenFull /\ ~OverflowInbox
  /\ Cardinality(Accepted) >= MaxInbox
  /\ UNCHANGED monVars

\* The counterfactual §5.3 forbids: making room by discarding what is queued.
QueueByDropping(i, j) ==
  /\ DropWhenFull
  /\ state[i] = "free"
  /\ ArrivedBelow(i)
  /\ j \in Accepted
  /\ state' = [state EXCEPT ![i] = "accepted", ![j] = "free"]
  /\ birth' = [birth EXCEPT ![i] = epoch]
  /\ everAccepted' = everAccepted \cup {i}
  /\ UNCHANGED <<epoch, log, appliedStale, drainedByOther, acceptedOverCap>>

\* The counterfactual of a bound at all.
QueueUnbounded(i) ==
  /\ OverflowInbox
  /\ state[i] = "free"
  /\ ArrivedBelow(i)
  /\ state' = [state EXCEPT ![i] = "accepted"]
  /\ birth' = [birth EXCEPT ![i] = epoch]
  /\ everAccepted' = everAccepted \cup {i}
  \* the defect itself: accepted although the inbox was already full
  /\ acceptedOverCap' = (acceptedOverCap \/ (Cardinality(Accepted) >= MaxInbox))
  /\ UNCHANGED <<epoch, log, appliedStale, drainedByOther>>

\* The recipient applies the lowest pending envelope of this epoch, in sequence
\* order. The append carries the envelope id as its dedup key, so an envelope
\* whose marker was lost appends nothing.
Apply(actor, i) ==
  /\ actor \in Actors
  /\ actor # "other" \/ AnyInstanceDrains
  /\ i \in Accepted
  /\ i = LowestPending
  /\ birth[i] = epoch
  /\ log' = IF i \in LogIds /\ ~ApplyWithoutDedup THEN log ELSE Append(log, i)
  /\ state' = [state EXCEPT ![i] = "applied"]
  /\ drainedByOther' = (drainedByOther \/ (actor = "other"))
  /\ UNCHANGED <<birth, epoch, everAccepted, appliedStale, acceptedOverCap>>

\* A stale-epoch envelope seals instead of leaking into the new epoch (A24).
Seal(actor, i) ==
  /\ actor \in Actors
  /\ actor # "other" \/ AnyInstanceDrains
  /\ i \in Accepted
  /\ i = LowestPending
  /\ birth[i] # epoch
  /\ state' = [state EXCEPT ![i] = "superseded"]
  /\ drainedByOther' = (drainedByOther \/ (actor = "other"))
  /\ UNCHANGED <<birth, epoch, log, everAccepted, appliedStale, acceptedOverCap>>

\* The counterfactual: the stale envelope is applied into the new epoch.
ApplyStale(actor, i) ==
  /\ ApplySealed
  /\ actor \in Actors
  /\ actor # "other" \/ AnyInstanceDrains
  /\ i \in Accepted
  /\ i = LowestPending
  /\ birth[i] # epoch
  /\ log' = Append(log, i)
  /\ state' = [state EXCEPT ![i] = "applied"]
  /\ appliedStale' = TRUE
  /\ drainedByOther' = (drainedByOther \/ (actor = "other"))
  /\ UNCHANGED <<birth, epoch, everAccepted, acceptedOverCap>>

\* The instance resets: its epoch bumps, so what is still queued becomes stale.
ResetEpoch ==
  /\ epoch = 0
  /\ epoch' = 1
  /\ UNCHANGED <<state, birth, log, everAccepted, appliedStale, drainedByOther, acceptedOverCap>>

\* The window the committed code closes with one transaction: the append
\* survived, the 'APPLIED' marker did not.
CrashLosesMarker(i) ==
  /\ state[i] = "applied"
  /\ i \in LogIds
  /\ state' = [state EXCEPT ![i] = "accepted"]
  /\ UNCHANGED <<birth, epoch, log, everAccepted, appliedStale, drainedByOther, acceptedOverCap>>

Stutter == UNCHANGED monVars

Next ==
  \/ \E i \in Ids : Queue(i)
  \/ \E i \in Ids : RefuseSend(i)
  \/ \E i, j \in Ids : QueueByDropping(i, j)
  \/ \E i \in Ids : QueueUnbounded(i)
  \/ \E actor \in Actors, i \in Ids : Apply(actor, i)
  \/ \E actor \in Actors, i \in Ids : Seal(actor, i)
  \/ \E actor \in Actors, i \in Ids : ApplyStale(actor, i)
  \/ ResetEpoch
  \/ \E i \in Ids : CrashLosesMarker(i)
  \/ Stutter

Init ==
  /\ state = [i \in Ids |-> "free"]
  /\ birth = [i \in Ids |-> 0]
  /\ epoch = 0
  /\ log = <<>>
  /\ everAccepted = {}
  /\ appliedStale = FALSE
  /\ drainedByOther = FALSE
  /\ acceptedOverCap = FALSE

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ state \in [Ids -> States]
  /\ birth \in [Ids -> Epochs]
  /\ epoch \in Epochs
  /\ log \in Seq(Ids)
  /\ everAccepted \subseteq Ids
  /\ appliedStale \in BOOLEAN
  /\ drainedByOther \in BOOLEAN
  /\ acceptedOverCap \in BOOLEAN

\* A06: an envelope's context change happens once, however often the drain is
\* replayed and even when the marker that would have skipped it was lost.
AtMostOncePerEnvelope == \A k, l \in 1..Len(log) : k # l => log[k] # log[l]

\* Applications follow the envelope sequence (the code's `ORDER BY sequence`).
LogIsIncreasing == \A k \in 1..(Len(log) - 1) : log[k] < log[k+1]

\* §5.3: nothing accepted is silently dropped — it is applied or sealed, and an
\* id that was ever accepted is never free again.
NoSilentLoss == \A i \in everAccepted : state[i] # "free"

\* A24: a stale-epoch envelope never reaches the new epoch's context.
NoStaleApplication == appliedStale = FALSE

\* §5.1's family: only the user or the instance itself drains its inbox.
DrainIsOwned == drainedByOther = FALSE

\* §5.3: no send is accepted into a full inbox — the bound is the decision at
\* insert (`queue_envelope` counts the ACCEPTED rows), which is what gives the
\* refusal its meaning; a lost marker may leave more rows pending than the cap.
BoundedInbox == acceptedOverCap = FALSE

=============================================================================
