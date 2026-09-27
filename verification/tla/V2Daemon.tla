------------------------------ MODULE V2Daemon -------------------------------
(***************************************************************************)
(* Session daemon protocol (plan §9/A28): stable client-chosen command ids,   *)
(* one read transaction for snapshot + watermark, events served after a       *)
(* watermark, and a slow client that never blocks the writer.                 *)
(*                                                                          *)
(* Code anchors: engine/src/v2/daemon.rs — `checkpoint` (read_snapshot and     *)
(* `watermark` inside ONE unchecked_transaction), `events` (sequence > since, *)
(* plus the current watermark and `resync_required`), PROTOCOL_VERSION         *)
(* handshake; core/src/v2/control.rs::submit_inner (command_id → payload_hash  *)
(* + stored result: a replay returns the stored receipt, a divergent replay is *)
(* refused).                                                                  *)
(*                                                                          *)
(* This version never reclaims events (daemon.rs header: "Events are never     *)
(* reclaimed in this first version"), so resync_required is always false - the *)
(* model expresses that with a `pruned` variable that no ordinary step sets,   *)
(* and states "the watermark never moves past the log" as an invariant.        *)
(*                                                                          *)
(* Counterfactual constants (D-219): each one TRUE is a plausible mistake — or *)
(* the feature the claim says this version does not have — and turns a claim   *)
(* that could not fail into one a configuration refutes. `ReplayRewritesReceipt*)
(* makes a same-payload replay re-apply the command and move its receipt,       *)
(* `RollbackLog` lets a compaction drop the oldest version and renumber the log *)
(* (`LogMonotone` is about exactly that), and `ReclaimEvents` reclaims events so *)
(* a client can need a resync (`NoResyncInThisVersion`). `drift`, `shrank` and  *)
(* `pruned` had no writer at all before this: every claim over them held for    *)
(* want of a step that could break it.                                         *)
(*                                                                          *)
(* D-248 (the session's stop lever) adds the daemon's *lifecycle* to this same  *)
(* model: a client asks this session to stop over the socket it is listening    *)
(* on (`teamagents daemon --stop`), the daemon *answers* that request — the     *)
(* reply is written by the serving task — and only then does the accept loop     *)
(* leave, taking the socket with it. Two claims come out of that order: a       *)
(* daemon that is gone has answered every stop request it took                  *)
(* (`StopsOnlyAfterAnswering`, so the client always has its receipt), and an    *)
(* answered stop *ends* the session (`AStopIsAnsweredAndEndsTheSession`, the    *)
(* liveness half). Their two counterfactuals, `StopBeforeReceipt` and           *)
(* `KeepServingAfterReceipt`, are the two mistakes in that order;               *)
(* `MC_daemon_stop.cfg` and its controls carry them.                            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Clients,     \* client slots, e.g. {"c1"}
          Commands,    \* command id slots, e.g. {"x1","x2"}
          Versions,    \* wire versions a client may offer, e.g. {1, 2}
          MaxLog,      \* event log bound (keeps the state space finite)
          ReplayRewritesReceipt,  \* counterfactual: a same-payload replay re-applies and moves the receipt
          RollbackLog,            \* counterfactual: a compaction drops the oldest version and renumbers
          ReclaimEvents,          \* counterfactual: events are reclaimed, so a resync can be needed
          StopBeforeReceipt,      \* counterfactual (D-248): the daemon leaves with a stop request unanswered
          KeepServingAfterReceipt \* counterfactual (D-248): an answered stop leaves the daemon serving

ASSUME Clients # {} /\ Commands # {} /\ Versions # {} /\ MaxLog > 0

Wire == 1              \* PROTOCOL_VERSION of the daemon
Payloads == {1, 2}     \* two distinguishable payloads per command id
noreceipt == 0         \* sentinel: the command never produced a receipt

VARIABLES
  logLength,    \* appended events so far: also the current state version
  applied,      \* command -> was it applied
  receipt,      \* command -> the version its stored receipt describes
  payloadOf,    \* command -> the payload that was applied
  versionOf,    \* command -> the wire version it arrived with
  snapshot,     \* client -> the version its snapshot is consistent with
  cursor,       \* client -> the event watermark the client holds
  view,         \* client -> the version the client has applied
  pruned,       \* whether the log lost events (v1: never)
  drift,        \* monitor: commands whose stored receipt changed (must stay {})
  shrank,       \* monitor: the log ever shrunk (must stay FALSE)
  serving,      \* the daemon is up and accepting requests (D-248)
  asked,        \* a client asked this session to stop over its socket (D-248)
  answered      \* that request has its reply on the wire (D-248)

vars == <<logLength, applied, receipt, payloadOf, versionOf, snapshot, cursor, view, pruned,
          serving, asked, answered>>
monVars == <<vars, drift, shrank>>
\* The protocol state (the log and its two monitors) and the D-248 lifecycle state. The two halves are
\* independent, so each action leaves the other half alone *by name* (a tuple no action writes is a constant of
\* the model, which is what the catalogue audit fails, D-219).
logVars == <<logLength, applied, receipt, payloadOf, versionOf, snapshot, cursor, view, pruned,
             drift, shrank>>
lifecycle == <<serving, asked, answered>>

\* ------------------------------------------------------------------- actions --
\* The runtime appends an event (a turn, a task settlement, a budget refusal…).
\* It is independent of every client cursor: a slow or disconnected client can
\* never stall the writer.
RuntimeEvent ==
  /\ logLength < MaxLog
  /\ logLength' = logLength + 1
  /\ shrank' = shrank
  /\ drift' = drift
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<applied, receipt, payloadOf, versionOf, snapshot, cursor, view, pruned>>

\* A client submits a command (§9): client-chosen id, stable across reconnects.
\* Only the advertised wire version is accepted (handshake), and an id that was
\* already applied is not applied twice.
Submit(c, cmd, payload, v) ==
  /\ v \in Versions
  /\ v = Wire
  /\ ~applied[cmd]
  /\ logLength < MaxLog
  /\ logLength' = logLength + 1
  /\ applied' = [applied EXCEPT ![cmd] = TRUE]
  /\ receipt' = [receipt EXCEPT ![cmd] = logLength + 1]
  /\ payloadOf' = [payloadOf EXCEPT ![cmd] = payload]
  /\ versionOf' = [versionOf EXCEPT ![cmd] = v]
  /\ shrank' = shrank
  /\ drift' = drift
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<snapshot, cursor, view, pruned>>

\* A replayed command id with the SAME payload returns the stored receipt and
\* changes nothing (that is what makes client retries safe).
Replay(c, cmd, payload) ==
  /\ applied[cmd] /\ payload = payloadOf[cmd]
  /\ UNCHANGED monVars

\* A replayed command id with a DIFFERENT payload is refused outright: the
\* stored receipt is authoritative, the state does not move.
ReplayDivergent(c, cmd, payload) ==
  /\ applied[cmd] /\ payload # payloadOf[cmd]
  /\ UNCHANGED monVars

\* Counterfactual (D-219): a same-payload replay re-applies the command — the
\* client retry that duplicates the effect — and the stored receipt moves with
\* it. `ReceiptsAreStable` is the claim about that, and `drift` records it.
ReapplyOnReplay(c, cmd, payload) ==
  /\ ReplayRewritesReceipt
  /\ applied[cmd] /\ payload = payloadOf[cmd]
  /\ logLength < MaxLog
  /\ logLength' = logLength + 1
  /\ receipt' = [receipt EXCEPT ![cmd] = logLength + 1]
  /\ drift' = drift \cup {cmd}
  /\ shrank' = shrank
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<applied, payloadOf, versionOf, snapshot, cursor, view, pruned>>

\* Counterfactual (D-219): a compaction drops the oldest event and renumbers the
\* log, so a version that named it is gone — the "versions are never reused or
\* rolled back" claim (`LogMonotone`) is what forbids this.
CompactLog ==
  /\ RollbackLog
  /\ logLength > 0
  /\ logLength' = logLength - 1
  /\ shrank' = TRUE
  /\ drift' = drift
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<applied, receipt, payloadOf, versionOf, snapshot, cursor, view, pruned>>

\* Counterfactual (D-219): events are reclaimed, so a client whose watermark
\* predates the reclaim does need a resync — the one thing `NoResyncInThisVersion`
\* says this version never asks for.
Reclaim(c) ==
  /\ ReclaimEvents
  /\ ~pruned
  /\ pruned' = TRUE
  /\ shrank' = shrank
  /\ drift' = drift
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<logLength, applied, receipt, payloadOf, versionOf, snapshot, cursor, view>>

\* checkpoint (§9): the snapshot and its watermark come from ONE read
\* transaction, so the client always learns exactly which version its snapshot
\* is consistent with.
Checkpoint(c) ==
  /\ snapshot' = [snapshot EXCEPT ![c] = logLength]
  /\ cursor' = [cursor EXCEPT ![c] = logLength]
  /\ view' = [view EXCEPT ![c] = logLength]
  /\ shrank' = shrank
  /\ drift' = drift
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<logLength, applied, receipt, payloadOf, versionOf, pruned>>

\* events(since = cursor): everything after the watermark, never a gap, because
\* this version never reclaims events.
Fetch(c) ==
  /\ cursor[c] <= logLength
  /\ ~pruned
  /\ cursor' = [cursor EXCEPT ![c] = logLength]
  /\ view' = [view EXCEPT ![c] = logLength]
  /\ shrank' = shrank
  /\ drift' = drift
  /\ UNCHANGED lifecycle
  /\ UNCHANGED <<logLength, applied, receipt, payloadOf, versionOf, snapshot, pruned>>

Stutter == UNCHANGED monVars

\* D-248: a client asks this session to stop over the socket it is listening on (`teamagents daemon --stop`).
\* The request is only a fact here — the *answer* is its own step, because the order of the two on the wire is
\* exactly what the claims below are about.
AskStop ==
  /\ serving
  /\ ~asked
  /\ asked' = TRUE
  /\ UNCHANGED <<serving, answered>>
  /\ UNCHANGED logVars

\* The answer is on the wire: `serve_client` writes the reply frame, and the flag that ends the accept loop is
\* set only after that write returns. A second answer would be a second reply to one request, so it is
\* refused by the guard.
AnswerStop ==
  /\ serving
  /\ asked
  /\ ~answered
  /\ answered' = TRUE
  /\ UNCHANGED <<serving, asked>>
  /\ UNCHANGED logVars

\* The accept loop leaves and takes the socket with it. The rule (D-248): it may not leave a stop request
\* unanswered — the reply is written first, and this is why a client that asked always has its receipt. The two
\* counterfactuals are the two ways to get that order wrong: leaving early (`StopBeforeReceipt`) and ignoring an
\* answered stop (`KeepServingAfterReceipt`, which is also what makes the liveness claim below refutable).
Leave ==
  /\ serving
  /\ ~(KeepServingAfterReceipt /\ answered)
  /\ (StopBeforeReceipt \/ ~asked \/ answered)
  /\ serving' = FALSE
  /\ UNCHANGED <<asked, answered>>
  /\ UNCHANGED logVars

Next ==
  \/ RuntimeEvent
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : \E v \in Versions :
        Submit(c, cmd, p, v)
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : Replay(c, cmd, p)
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : ReplayDivergent(c, cmd, p)
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : ReapplyOnReplay(c, cmd, p)
  \/ CompactLog
  \/ \E c \in Clients : Reclaim(c)
  \/ \E c \in Clients : Checkpoint(c)
  \/ \E c \in Clients : Fetch(c)
  \/ AskStop
  \/ AnswerStop
  \/ Leave
  \/ Stutter

Init ==
  /\ logLength = 0
  /\ applied = [ cmd \in Commands |-> FALSE ]
  /\ receipt = [ cmd \in Commands |-> noreceipt ]
  /\ payloadOf = [ cmd \in Commands |-> 0 ]
  /\ versionOf = [ cmd \in Commands |-> 0 ]
  /\ snapshot = [ c \in Clients |-> 0 ]
  /\ cursor = [ c \in Clients |-> 0 ]
  /\ view = [ c \in Clients |-> 0 ]
  /\ pruned = FALSE
  /\ drift = {}
  /\ shrank = FALSE
  /\ serving = TRUE
  /\ asked = FALSE
  /\ answered = FALSE

\* The accept loop is a running task: while a stop request is waiting to be answered it is busy answering it, and
\* once answered the loop really does leave (`WF_vars(Leave)` is the driver's poll loop, which cannot sit on a
\* set flag forever — engine/src/v2/daemon.rs).
Spec == Init /\ [][Next]_monVars /\ WF_vars(Leave)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ logLength \in 0..MaxLog
  /\ \A cmd \in Commands : applied[cmd] \in BOOLEAN
  /\ \A cmd \in Commands : receipt[cmd] \in 0..MaxLog
  /\ \A cmd \in Commands : payloadOf[cmd] \in Payloads \cup {0}
  /\ \A cmd \in Commands : versionOf[cmd] \in Versions \cup {0}
  /\ \A c \in Clients : snapshot[c] \in 0..MaxLog
  /\ \A c \in Clients : cursor[c] \in 0..MaxLog
  /\ \A c \in Clients : view[c] \in 0..MaxLog
  /\ pruned \in BOOLEAN
  /\ serving \in BOOLEAN
  /\ asked \in BOOLEAN
  /\ answered \in BOOLEAN

\* the event log only grows: versions are never reused or rolled back
LogMonotone == shrank = FALSE

\* A28 "command dedup": a command id is applied at most once, and its stored receipt
\* is stable — replays (same payload) return that receipt, divergent replays are
\* refused rather than applied again
AppliedAtMostOnce == \A cmd \in Commands : applied[cmd] => receipt[cmd] # noreceipt

ReceiptsAreStable == drift = {}

\* ... and every receipt names a version that really exists
ReceiptNamesARealVersion ==
  \A cmd \in Commands : receipt[cmd] # noreceipt => receipt[cmd] \in 1..logLength

\* the handshake: only commands offered with the advertised wire version run
AppliedCommandsUsedTheWireVersion ==
  \A cmd \in Commands : applied[cmd] => versionOf[cmd] = Wire

\* A28 "snapshot watermark correctness": the snapshot never claims to be newer than the watermark
\* the client holds for it — that is what "snapshot + watermark from one read
\* transaction" buys, and a state where the snapshot leads the cursor is
\* unreachable
SnapshotNeverLeadsCursor == \A c \in Clients : snapshot[c] <= cursor[c]

\* A28 "no gaps": the client's view is exactly the version its cursor claims,
\* and neither runs past the log (this version never reclaims events, so a
\* resync is never needed)
ViewMatchesCursor == \A c \in Clients : view[c] = cursor[c]
CursorNeverBeyondLog == \A c \in Clients : cursor[c] <= logLength
NoResyncInThisVersion == ~pruned

\* D-248: a daemon that is *gone* has answered every stop request it took. That is the receipt rule the code
\* keeps by writing the reply before it sets the flag that ends the accept loop — the client that asked is
\* never left with a broken connection instead of an answer.
StopsOnlyAfterAnswering == serving = FALSE => (asked => answered)

\* --------------------------------------------------------------- properties --
\* D-248, the liveness half: an answered stop *ends* the session. Safety alone would be satisfied by a daemon
\* that answers and then serves on forever, which is exactly the shape the first wiring of the lever had (the
\* process sat in its signal wait with its socket already gone, measured 2026-09-27). The `[]` is not
\* decoration: without it TLC evaluates the implication at the *initial* state, where `asked` is FALSE and the
\* property is vacuously true — measured 2026-09-27, when the control below verified instead of refuting.
AStopIsAnsweredAndEndsTheSession == []( (asked /\ answered) => <>(serving = FALSE) )

=============================================================================
