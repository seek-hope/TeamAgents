------------------------------ MODULE V2Sessions ----------------------------
(***************************************************************************)
(* Named sessions under one base (D-364).                                    *)
(*                                                                           *)
(* A registry maps session ids to directories; each directory is one state   *)
(* root. What this model adds is the registry's own rules and the lifecycle   *)
(* guards around it:                                                          *)
(*                                                                           *)
(*   - two ids never name one path (`RegistryIsInjective`): two sessions in   *)
(*     one state root is exactly what A33 forbids. The per-path coordinator   *)
(*     lock itself is not restated here — it is `V2Coordinator`'s            *)
(*     `AtMostOneCoordinator` for one root, and with injectivity the roots    *)
(*     of two sessions are different, so their locks cannot be one;           *)
(*   - a directory is never archived while a coordinator holds it            *)
(*     (`ArchiveOnlyWhenIdle`, the code's `refuse_if_live`);                  *)
(*   - an archived session is never held (`NoCoordinatorForAnArchivedSession`, *)
(*     the state half of the same rule); and                                  *)
(*   - the default session is never archived (`DefaultIsNeverArchived`).      *)
(*                                                                           *)
(* The default session is always registered on `p1` (in the code it is the    *)
(* base directory itself and needs no record); `New` only creates *named*     *)
(* sessions. Ceiling: safety only — a session need not ever be opened, and    *)
(* the filesystem effects (a directory that really moves) are the code's,     *)
(* pinned by the `sessions` unit and CLI tests. The three negative controls   *)
(* at the end each forget one rule and must be refuted.                       *)
(*                                                                           *)
(* Code anchors: engine/src/v2/sessions.rs (the registry, `resolve`,          *)
(* `archive`, `delete`, `refuse_if_live`), with the unit tests in that file   *)
(* and `cli::named_sessions_are_created_resolved_and_retired`.                *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Ids,               \* session ids, e.g. {"default","a","b"}
          Paths,             \* directories a session may own, e.g. {"p1","p2","p3"}
          AllowSharedPath,   \* negative control: New may map a second id onto a used path
          ArchiveWhileHeld,  \* negative control: Archive ignores the coordinator
          AllowDefaultArchive \* negative control: Archive may move the default session

ASSUME "default" \in Ids /\ "p1" \in Paths

VARIABLES
  reg,      \* the registry: id -> its path, or "none" for an unregistered id
  holder,   \* the coordinators: a set of [id, path] records
  archived  \* ids whose directory was moved aside; they are not attachable

vars == <<reg, holder, archived>>

\* ------------------------------------------------------------------ actions --
\* Create a named session directory and record it. The default is never created here, and a path may not be
\* reused by a second id unless the counterfactual forgets that rule.
New(id, p) ==
  /\ id \in Ids
  /\ id # "default"
  /\ p \in Paths
  /\ reg[id] = "none"
  /\ (AllowSharedPath \/ \A j \in Ids : reg[j] # p)
  /\ reg' = [reg EXCEPT ![id] = p]
  /\ UNCHANGED <<holder, archived>>

\* Attach a coordinator to a session's own path. A held path is not taken again, and an archived session is
\* not attachable.
Open(id) ==
  /\ reg[id] # "none"
  /\ ~(id \in archived)
  /\ \A e \in holder : e.path # reg[id]
  /\ holder' = holder \union {[id |-> id, path |-> reg[id]]}
  /\ UNCHANGED <<reg, archived>>

Close(id) ==
  /\ \E e \in holder : e.id = id
  /\ holder' = {e \in holder : e.id # id}
  /\ UNCHANGED <<reg, archived>>

\* Move a session's directory aside. Refused for the default and while a coordinator holds it, unless a
\* counterfactual forgets the guard.
Archive(id) ==
  /\ reg[id] # "none"
  /\ ~(id \in archived)
  /\ (AllowDefaultArchive \/ id # "default")
  /\ (ArchiveWhileHeld \/ \A e \in holder : e.id # id)
  /\ archived' = archived \union {id}
  /\ UNCHANGED <<reg, holder>>

Stutter == UNCHANGED vars

Next ==
  \/ \E i \in Ids, p \in Paths : New(i, p)
  \/ \E i \in Ids : Open(i)
  \/ \E i \in Ids : Close(i)
  \/ \E i \in Ids : Archive(i)
  \/ Stutter

Init ==
  /\ reg = [ i \in Ids |-> IF i = "default" THEN "p1" ELSE "none" ]
  /\ holder = {}
  /\ archived = {}

Spec == Init /\ [][Next]_vars

\* -------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A i \in Ids : reg[i] \in Paths \union {"none"}
  /\ \A e \in holder : e.id \in Ids /\ e.path \in Paths
  /\ archived \subseteq Ids

\* A33: one state root per session — no two ids may name one path.
RegistryIsInjective ==
  \A i, j \in Ids : (reg[i] # "none" /\ reg[i] = reg[j]) => i = j

\* An archived session is not attachable, so nothing holds it.
NoCoordinatorForAnArchivedSession ==
  \A e \in holder : ~(e.id \in archived)

\* The base directory's own session is never moved aside.
DefaultIsNeverArchived ==
  "default" \notin archived

\* -------------------------------------------------------------- properties --
\* The rule, at the step that archives: the session had no coordinator.
ArchiveOnlyWhenIdle ==
  [][ \A i \in Ids :
        (i \notin archived /\ i \in archived') =>
          (\A e \in holder : e.id # i) ]_vars

=============================================================================
