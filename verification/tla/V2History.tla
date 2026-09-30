------------------------------ MODULE V2History ----------------------------
(***************************************************************************)
(* Private history and who may read it (§5.1/§5.4, A05, Q8).               *)
(*                                                                         *)
(* "The user may read any instance's history; that grants no equivalent    *)
(*  permission to other agents." (DESIGN §1, Q8) The control entry states  *)
(* the rule in code: `read_history` accepts `Identity::User`, accepts an   *)
(* instance reading *its own* history, refuses every other instance with   *)
(* "may not read the private history of …", and refuses `Identity::System` *)
(* outright — a member-facing surface issues the command today, so the     *)
(* refusal is what keeps the isolation real rather than advertised.        *)
(*                                                                         *)
(* What the model adds to the sentence is that the rule holds over *every* *)
(* interleaving of reads and actors, including a session with more than one *)
(* instance: a state variable records each successful read, and the two    *)
(* claims below say a recorded read is the user's or the reader's own, and *)
(* that the runtime itself never appears as a reader. A refused read       *)
(* records nothing — its action is simply not enabled — so `reads` is      *)
(* exactly the set of reads that happened. The set is finite (actors times *)
(* instances), so the enumeration is finite without a log-length bound.    *)
(*                                                                         *)
(* Ceiling: safety only (what could be read), no liveness (a read need not *)
(* happen). The command also refuses a target from another session; that   *)
(* session boundary is `V2Store`'s identity and the transaction's own      *)
(* `session_id` comparison, not modelled here. The negative controls at the *)
(* end are the two plausible bugs — an instance allowed to read a          *)
(* stranger's history, and a runtime allowed to read — and each must make  *)
(* TLC refute the matching property, which keeps the rule non-vacuous.     *)
(*                                                                         *)
(* Code anchors: core/src/v2/control.rs `read_history` (the three-branch   *)
(* identity match), pinned by the control-plane test                       *)
(* `read_history_is_user_or_self_only` (A05).                              *)
(***************************************************************************)
EXTENDS FiniteSets

CONSTANTS Instances,    \* the session's instances, e.g. {"i1","i2"}
          AllowForeign, \* negative control: an instance may read another's history
          AllowSystem   \* negative control: the runtime may read a member's history

Actors == Instances \union {"user", "system"}
Reads == [reader : Actors, target : Instances]

VARIABLES
  reads   \* the successful reads so far: a set of [reader, target] records

vars == <<reads>>

\* ------------------------------------------------------------------ actions --
\* One actor reads one instance's history. The guard *is* the rule: the user
\* may read any target; an instance only its own; the system never. A refused
\* read cannot take this step, so nothing is recorded for it.
Read(reader, target) ==
  /\ reader \in Actors
  /\ target \in Instances
  /\ ( reader = "user"
       \/ (reader \in Instances /\ (reader = target \/ AllowForeign))
       \/ (reader = "system" /\ AllowSystem) )
  /\ reads' = reads \union {[reader |-> reader, target |-> target]}

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED vars

Next == ( \E r \in Actors, t \in Instances : Read(r, t) ) \/ Stutter

Init == reads = {}

Spec == Init /\ [][Next]_vars

\* -------------------------------------------------------------- invariants --
TypeOK ==
  reads \subseteq Reads

\* Q8/A05: an instance may read only its own history. (The user's clause is
\* deliberately not a claim: "the user may read any" is the absence of a
\* restriction, so it has no state half to break.)
InstancesReadOnlyTheirOwn ==
  \A r \in reads : r.reader \in Instances => r.reader = r.target

\* §5.1: the runtime itself never reads a member's private history.
SystemNeverReads ==
  \A r \in reads : r.reader # "system"

=============================================================================
