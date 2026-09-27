------------------------------ MODULE V2Retention ----------------------------
(***************************************************************************)
(* Retention as DESIGN states it (§4.4, §9; D-75 recorded the keys as      *)
(* accepted-but-unapplied, D-192 models the rule before an implementation).*)
(*                                                                         *)
(* "Ordinary history is archived or cleaned per user configuration ...     *)
(*  while live references and evaluation evidence are never evicted"       *)
(* (§9), and `[retention] history_days` says which facts are ordinary:     *)
(* "Drop applied deliveries and events older than this many days ... 0     *)
(*  keeps the full history: events are the audit trail." (the struct)      *)
(*                                                                         *)
(* What the model adds to the sentence is the *interleaving*, which is why *)
(* a model is worth having before the destructive code is written: a live  *)
(* reference can attach to a fact, detach again, and evaluation evidence   *)
(* can be marked, all while days pass and the sweep picks facts. The rule  *)
(* is therefore stated as a property of the step that evicts — retention   *)
(* switched on, the fact old enough, nothing protecting it *at that        *)
(* moment* — plus the state claims that a reference and an evidence mark   *)
(* always imply the fact is still there.                                   *)
(*                                                                         *)
(* Ceiling: safety only. The design does not say a configured cleanup must *)
(* eventually run ("scheduled separately", §4.4), and with references able *)
(* to attach forever "eventually evicted" is not a property of every       *)
(* behaviour, so no liveness property is stated. The four negative         *)
(* controls at the end are the pre-implementation behaviours: each forgets *)
(* one guard and must make TLC refute the matching property, which is what *)
(* keeps the guards from being vacuous.                                    *)
(*                                                                         *)
(* Code anchors (once implemented): core/v2/control.rs (a sweep would be a *)
(* control command deleting rows in a transaction, like `artifact_collect` *)
(* in D-191), engine/src/config.rs (`Retention::archived_days` /           *)
(* `history_days`), engine/src/cli.rs (the `doctor` row that today reports *)
(* the keys as not applied).                                               *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Facts,      \* the session's prunable facts, e.g. {"e1","e2","e3"}
          Live0,      \* facts a live reference protects when the session opens
          Evidence0,  \* facts that are already evaluation evidence
          Horizon,    \* `history_days`: facts this old are ordinary; 0 disables retention
          MaxAge,     \* the age bound TLC needs (ages stop here)
          IgnoreLive,     \* negative control: the sweep forgets the live-reference guard
          IgnoreEvidence, \* negative control: the sweep forgets the evaluation-evidence guard
          IgnoreAge,      \* negative control: the sweep forgets the horizon
          IgnoreDisabled  \* negative control: the sweep runs with retention switched off

ASSUME Facts # {}
    /\ Live0 \subseteq Facts
    /\ Evidence0 \subseteq Facts
    /\ Horizon \in 0..MaxAge

VARIABLES
  age,       \* fact -> age in days, bounded by MaxAge
  present,   \* fact -> whether the fact is still in the database
  live,      \* fact -> a live reference currently protects it (an open goal's receipts, a running operation)
  evidence   \* fact -> it is part of a recorded evaluation

vars == <<age, present, live, evidence>>

\* ------------------------------------------------------------------ actions --
\* One day passes: facts still present age, up to the bound the model needs.
Tick ==
  /\ age' = [ f \in Facts |->
                IF present[f] /\ age[f] < MaxAge THEN age[f] + 1 ELSE age[f] ]
  /\ UNCHANGED <<present, live, evidence>>

\* The sweep drops one fact — and this is the whole rule: it must be present,
\* retention must be switched on (Horizon # 0), it must be old enough, and
\* nothing may protect it. The negative controls set the Ignore* constants to
\* forget one guard each, and the property below is stated on the step so the
\* guards are checked *at the moment of eviction*, not only in the end state.
Sweep(f) ==
  /\ present[f]
  /\ (IgnoreDisabled \/ Horizon # 0)
  /\ (IgnoreAge \/ age[f] >= Horizon)
  /\ (IgnoreLive \/ ~live[f])
  /\ (IgnoreEvidence \/ ~evidence[f])
  /\ present' = [present EXCEPT ![f] = FALSE]
  \* the protection facts are left as they were: the sweep never "cleans up" the
  \* reference it ignored, which is what makes `NoReferenceToEvictedFact` the
  \* invariant that catches a sweep without the guard (measured in the control)
  /\ UNCHANGED <<age, live, evidence>>

\* A reference attaches to a fact that is still there — the step that makes a
\* fact ordinary history protected, possibly long after it was written.
AttachReference(f) ==
  /\ present[f]
  /\ ~live[f]
  /\ live' = [live EXCEPT ![f] = TRUE]
  /\ UNCHANGED <<age, present, evidence>>

\* The goal holding it closes: the fact becomes evictable again if it is old.
DetachReference(f) ==
  /\ live[f]
  /\ live' = [live EXCEPT ![f] = FALSE]
  /\ UNCHANGED <<age, present, evidence>>

\* A run is recorded as evaluation evidence: from that step on it is never evicted.
MarkEvidence(f) ==
  /\ present[f]
  /\ ~evidence[f]
  /\ evidence' = [evidence EXCEPT ![f] = TRUE]
  /\ UNCHANGED <<age, present, live>>

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED vars

Next ==
  \/ Tick
  \/ \E f \in Facts : Sweep(f)
  \/ \E f \in Facts : AttachReference(f)
  \/ \E f \in Facts : DetachReference(f)
  \/ \E f \in Facts : MarkEvidence(f)
  \/ Stutter

Init ==
  /\ age = [ f \in Facts |-> 0 ]
  /\ present = [ f \in Facts |-> TRUE ]
  /\ live = [ f \in Facts |-> f \in Live0 ]
  /\ evidence = [ f \in Facts |-> f \in Evidence0 ]

Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A f \in Facts : age[f] \in 0..MaxAge
  /\ \A f \in Facts : present[f] \in BOOLEAN
  /\ \A f \in Facts : live[f] \in BOOLEAN
  /\ \A f \in Facts : evidence[f] \in BOOLEAN

\* §9: a live reference to a fact that no longer exists is the corruption the
\* rule exists to prevent — so this holds in every reachable state.
NoReferenceToEvictedFact == \A f \in Facts : live[f] => present[f]

\* §9: evaluation evidence is never evicted, however old it is.
EvidenceIsNeverEvicted == \A f \in Facts : evidence[f] => present[f]

\* The state half of the rule: a fact that is gone was old, and retention was on.
\* (The step half — nothing protected it *when* it went — is the property below.)
OnlyOldFactsAreEvicted ==
  \A f \in Facts : ~present[f] => (Horizon # 0 /\ age[f] >= Horizon)

\* --------------------------------------------------------------- properties --
\* The rule, at the step that evicts: these are the guards, evaluated in the
\* state the fact left from. Each negative control forgets one and is refuted.
EvictionOnlyUnderTheGuards ==
  [][ \A f \in Facts :
        (present[f] /\ ~present'[f]) =>
          (Horizon # 0) /\ (age[f] >= Horizon) /\ ~live[f] /\ ~evidence[f] ]_vars

=============================================================================
