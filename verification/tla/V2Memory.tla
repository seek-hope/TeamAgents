------------------------------ MODULE V2Memory -------------------------------
(***************************************************************************)
(* Durable memory (D-405): one append-only note store at the state root, which   *)
(* every session under that root recalls from. Two claims are the module's       *)
(* whole content, and both are about what a *later* reader can rely on:          *)
(*                                                                          *)
(*   NotesAreAppendOnly — a note, once stored, is never rewritten or evicted.    *)
(*     A recall that answered one way keeps answering that way, which is what    *)
(*     makes the store worth writing to; refusing at the cap (the code's rule)   *)
(*     instead of evicting the oldest is exactly this claim.                     *)
(*   RecallIsBounded — a recall returns at most RecallMax notes, whatever the    *)
(*     caller asks for, so a store that grows cannot grow a context without      *)
(*     bound.                                                                    *)
(*                                                                          *)
(* Code anchors: engine/src/v2/memory.rs — Memory::remember (refuses over        *)
(* NOTE_MAX_CHARS and at MAX_NOTES rather than evicting; appends and persists in *)
(* one step) and Memory::recall (newest first, `limit.clamp(1, RECALL_MAX)`);    *)
(* engine/src/tools.rs — memory_tool (the `remember`/`recall` actions, one        *)
(* process-wide write lock) under the `memory` binding.                          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, Sequences

CONSTANTS MaxNotes,   \* the store's cap (the code's MAX_NOTES)
          RecallMax,  \* the most notes one recall returns (RECALL_MAX)
          Evicts      \* counterfactual (D-405): at the cap, drop the oldest to make room

ASSUME MaxNotes > 0 /\ RecallMax > 0

VARIABLES
  notes,     \* the ids the store holds
  freshest,  \* note ids in store order (oldest first) — what "newest first" is read from
  result,    \* the notes the most recent recall returned
  recalled,  \* monitor: a recall ran
  lost       \* monitor: a note ever left the store

vars == <<notes, freshest, result>>
monVars == <<vars, recalled, lost>>

\* --------------------------------------------------------------- actions --
\* `remember`: a new id is appended with a fresh sequence number. Refused at the cap.
Remember(n) ==
  /\ n \notin notes
  /\ Cardinality(notes) < MaxNotes
  /\ notes' = notes \cup {n}
  /\ freshest' = Append(freshest, n)
  /\ result' = result
  /\ recalled' = recalled
  /\ lost' = lost

\* The counterfactual: make room by dropping the oldest, which is what the code refuses to do.
EvictOldest ==
  /\ Evicts
  /\ Cardinality(notes) >= MaxNotes
  /\ freshest # <<>>
  /\ notes' = notes \ {Head(freshest)}
  /\ freshest' = Tail(freshest)
  /\ result' = result
  /\ recalled' = recalled
  /\ lost' = TRUE

\* `recall`: at most RecallMax of the newest notes, none invented.
Recall(q) ==
  /\ q \subseteq notes
  /\ Cardinality(q) <= RecallMax
  /\ result' = q
  /\ recalled' = TRUE
  /\ notes' = notes
  /\ freshest' = freshest
  /\ lost' = lost

Stutter == UNCHANGED monVars

Next ==
  \/ \E n \in 0..MaxNotes : Remember(n)
  \/ EvictOldest
  \/ \E q \in SUBSET notes : Recall(q)
  \/ Stutter

Init ==
  /\ notes = {}
  /\ freshest = <<>>
  /\ result = {}
  /\ recalled = FALSE
  /\ lost = FALSE

Spec == Init /\ [][Next]_monVars

\* ------------------------------------------------------------- invariants --
TypeOK ==
  /\ notes \subseteq 0..MaxNotes
  /\ \A n \in notes : \E i \in DOMAIN freshest : freshest[i] = n
  /\ result \subseteq notes
  /\ Cardinality(notes) <= MaxNotes

\* D-405: nothing ever leaves the store — the memory a session wrote is the memory the next one reads.
NotesAreAppendOnly ==
  lost = FALSE

\* ... and the store stays within its cap only because `remember` is refused at it, never by eviction.
TheStoreRefusesAtTheCap ==
  Cardinality(notes) <= MaxNotes

\* D-405: a recall returns at most RecallMax notes and only real ones.
RecallIsBounded ==
  recalled => /\ Cardinality(result) <= RecallMax
              /\ result \subseteq notes

=============================================================================
