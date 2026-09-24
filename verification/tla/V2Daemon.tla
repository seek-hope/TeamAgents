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
(* 这一版事件从不回收（daemon.rs 头注："Events are never reclaimed in this     *)
(* first version"），所以 resync_required 恒为 false——模型用一个从不置真的   *)
(* `pruned` 变量如实表达，并把"水位不会越过日志"写成不变量。                  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Clients,     \* client slots, e.g. {"c1"}
          Commands,    \* command id slots, e.g. {"x1","x2"}
          Versions,    \* wire versions a client may offer, e.g. {1, 2}
          MaxLog       \* event log bound (keeps the state space finite)

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
  shrank        \* monitor: the log ever shrunk (must stay FALSE)

vars == <<logLength, applied, receipt, payloadOf, versionOf, snapshot, cursor, view, pruned>>
monVars == <<vars, drift, shrank>>

\* ------------------------------------------------------------------- actions --
\* The runtime appends an event (a turn, a task settlement, a budget refusal…).
\* It is independent of every client cursor: a slow or disconnected client can
\* never stall the writer.
RuntimeEvent ==
  /\ logLength < MaxLog
  /\ logLength' = logLength + 1
  /\ shrank' = shrank
  /\ drift' = drift
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

\* checkpoint (§9): the snapshot and its watermark come from ONE read
\* transaction, so the client always learns exactly which version its snapshot
\* is consistent with.
Checkpoint(c) ==
  /\ snapshot' = [snapshot EXCEPT ![c] = logLength]
  /\ cursor' = [cursor EXCEPT ![c] = logLength]
  /\ view' = [view EXCEPT ![c] = logLength]
  /\ shrank' = shrank
  /\ drift' = drift
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
  /\ UNCHANGED <<logLength, applied, receipt, payloadOf, versionOf, snapshot, pruned>>

Stutter == UNCHANGED monVars

Next ==
  \/ RuntimeEvent
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : \E v \in Versions :
        Submit(c, cmd, p, v)
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : Replay(c, cmd, p)
  \/ \E c \in Clients : \E cmd \in Commands : \E p \in Payloads : ReplayDivergent(c, cmd, p)
  \/ \E c \in Clients : Checkpoint(c)
  \/ \E c \in Clients : Fetch(c)
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

Spec == Init /\ [][Next]_monVars

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

\* the event log only grows: versions are never reused or rolled back
LogMonotone == shrank = FALSE

\* A28 "命令去重": a command id is applied at most once, and its stored receipt
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

\* A28 "快照水位正确": the snapshot never claims to be newer than the watermark
\* the client holds for it — that is what "snapshot + watermark from one read
\* transaction" buys, and a state where the snapshot leads the cursor is
\* unreachable
SnapshotNeverLeadsCursor == \A c \in Clients : snapshot[c] <= cursor[c]

\* A28 "无缺口": the client's view is exactly the version its cursor claims,
\* and neither runs past the log (this version never reclaims events, so a
\* resync is never needed)
ViewMatchesCursor == \A c \in Clients : view[c] = cursor[c]
CursorNeverBeyondLog == \A c \in Clients : cursor[c] <= logLength
NoResyncInThisVersion == ~pruned

=============================================================================
