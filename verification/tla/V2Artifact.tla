------------------------------ MODULE V2Artifact -----------------------------
(***************************************************************************)
(* Artifact lifecycle model (plan §4.3, A30).                              *)
(* Code anchors: core/v2/control.rs artifact_stage / artifact_publish /    *)
(* publish_list / artifact_gc_claim; engine/v2/driver.rs                   *)
(* store_response_artifact (tmp file -> fsync -> rename -> stage).         *)
(*                                                                         *)
(* The point is the *interleaving*: bytes are persisted before the row is  *)
(* referenced, a reference and the LIVE flip commit together, and GC may   *)
(* only claim an artifact that no live owner references.                   *)
(*                                                                         *)
(* Counterfactual constant (D-220): `GcIgnoresHolders` TRUE is the collector  *)
(* that never reads the reference table — the mistake `GcClaimsOnlyUnreferenced*)
(* Live` exists to catch, and the only thing wrong in its control.           *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Artifacts,  \* artifact slots, e.g. {"a1","a2"}
          Owners,     \* reference owners, e.g. {"r1","r2"}
          Spent,      \* the artifacts whose staging owner has finished (D-436): a finished owner holds nothing
          Aged,       \* ...and which are older than the `[retention] history_days` window: ordinary history
          GcIgnoresHolders,  \* counterfactual: GC claims an artifact that still has references
          ReleaseLiveOwner,  \* counterfactual: a hold is released although its owner has not finished
          ReleaseFreshArtifact, \* counterfactual: a hold is released inside the retention window
          \* D-462 counterfactuals, one per claim:
          ReferenceUnpersisted, \* a reference attaches while the bytes are absent
          ReleaseDeletesBytes,  \* dropping a hold also deletes the bytes
          CollectorKeepsGhost,  \* the collector leaves the row behind
          ReferenceToStaging,   \* a reference attaches to a STAGING artifact
          DeleteAnyone,         \* bytes are deleted from a row that is not DELETING
          ClaimAnyone,          \* GC claims a row that is not LIVE
          LiveWithoutReference  \* the LIVE flip carries no reference

ASSUME Artifacts # {} /\ Owners # {}

States == {"none", "STAGING", "LIVE", "DELETING", "ABANDONED"}
Disk == {"absent", "present"}

VARIABLES
  row,      \* artifact -> lifecycle state in the database
  disk,     \* artifact -> whether the bytes are durably on disk
  holders,  \* artifact -> set of live reference owners (bounded on purpose)
  owner,    \* artifact -> whether a publishing owner is still protecting it
  staged,   \* artifact -> the tmp file was written but the row may not exist
  released  \* artifact -> the holds released so far (a monitor for the release rule)

vars == <<row, disk, holders, owner, staged, released>>

\* ------------------------------------------------------------------- actions --
\* 1) bytes first: tmp file, fsync, rename (a crash here leaves an orphan file)
WriteBytes(id) ==
  /\ disk[id] = "absent"
  /\ staged[id] = FALSE
  /\ disk' = [disk EXCEPT ![id] = "present"]
  /\ staged' = [staged EXCEPT ![id] = TRUE]
  /\ UNCHANGED <<row, holders, owner, released>>

\* 2) then the STAGING row with its owner (crash between 1 and 2 = orphan file)
StageRow(id) ==
  /\ staged[id] /\ row[id] = "none"
  /\ row' = [row EXCEPT ![id] = "STAGING"]
  /\ owner' = [owner EXCEPT ![id] = TRUE]
  /\ UNCHANGED <<disk, holders, staged, released>>

\* 3) the referencing transaction: LIVE flip and the reference commit together
PublishWithReference(id, who) ==
  /\ (ReferenceToStaging \/ row[id] = "STAGING") /\ (ReferenceUnpersisted \/ disk[id] = "present")
  /\ who \in Owners \ holders[id]
  /\ row' = [row EXCEPT ![id] = IF ReferenceToStaging THEN "STAGING" ELSE "LIVE"]
  /\ holders' = [holders EXCEPT ![id] = IF LiveWithoutReference THEN holders[id] ELSE @ \cup {who}]
  /\ owner' = [owner EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<disk, staged, released>>

\* D-462 counterfactual: the bytes of a LIVE artifact disappear while the row stays.
GhostDelete(id) ==
  /\ CollectorKeepsGhost
  /\ disk[id] = "present"
  /\ disk' = [disk EXCEPT ![id] = "absent"]
  /\ UNCHANGED <<row, holders, owner, staged, released>>

\* D-462 counterfactual: a row goes LIVE and takes a reference although no bytes were ever written.
PublishWithoutBytes(id, who) ==
  /\ ReferenceUnpersisted
  /\ row[id] = "none" /\ disk[id] = "absent"
  /\ who \in Owners \ holders[id]
  /\ row' = [row EXCEPT ![id] = "LIVE"]
  /\ holders' = [holders EXCEPT ![id] = @ \cup {who}]
  /\ UNCHANGED <<disk, owner, staged, released>>

\* add another reference to an already live artifact (new owner attaches)
AddReference(id, who) ==
  /\ row[id] = "LIVE" /\ (ReferenceUnpersisted \/ disk[id] = "present")
  /\ who \in Owners \ holders[id]
  /\ holders' = [holders EXCEPT ![id] = @ \cup {who}]
  /\ UNCHANGED <<row, disk, owner, staged, released>>

\* GC claim: only ownerless, unreferenced LIVE artifacts become DELETING
GcClaim(id) ==
  /\ (ClaimAnyone \/ row[id] = "LIVE") /\ (GcIgnoresHolders \/ holders[id] = {}) /\ ~owner[id]
  /\ row' = [row EXCEPT ![id] = "DELETING"]
  /\ UNCHANGED <<disk, holders, owner, staged, released>>

\* GC delete: the file goes away, the row leaves the catalog
GcDelete(id) ==
  /\ (DeleteAnyone \/ row[id] = "DELETING") /\ holders[id] = {}
  /\ disk' = [disk EXCEPT ![id] = "absent"]
  /\ row' = [row EXCEPT ![id] = IF CollectorKeepsGhost THEN "LIVE" ELSE "none"]
  /\ staged' = [staged EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<holders, owner, released>>

\* orphan cleanup: a STAGING artifact whose publishing request died is abandoned
Abandon(id) ==
  /\ row[id] = "STAGING"
  /\ row' = [row EXCEPT ![id] = "ABANDONED"]
  /\ owner' = [owner EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<disk, holders, staged, released>>

\* D-436: a dropped owner stops protecting its artifact — for an aborted publish, and (the half the product was
\* missing) when the staging owner has **finished**: `Spent` is the product's own rule, a request that is not
\* PENDING or an operation whose status is terminal but not OUTCOME_UNKNOWN (that one's runner may still verify).
\* The release touches only the hold: `row`, `disk` and `holders` are unchanged, so `GcClaim` still decides what
\* may go. Before D-436 nothing in the product ever dropped a hold, which is why the model's `~owner[id]` had no
\* counterpart in the code (the D-253 probe measured the consequence: 3 LIVE artifacts, 0 claimable).
DropOwner(id) ==
  /\ owner[id]
  /\ (ReleaseLiveOwner \/ id \in Spent)
  /\ (ReleaseFreshArtifact \/ id \in Aged)
  /\ owner' = [owner EXCEPT ![id] = FALSE]
  /\ released' = released \cup (IF id \in Spent /\ id \in Aged THEN {} ELSE {id})
  /\ disk' = [disk EXCEPT ![id] = IF ReleaseDeletesBytes THEN "absent" ELSE disk[id]]
  /\ UNCHANGED <<row, holders, staged>>

\* ---------------------------------------------------------------------- spec --
\* an idle step keeps the model open-ended (TLC then reports no false deadlock)
Stutter == UNCHANGED vars

Next ==
  \/ \E id \in Artifacts : WriteBytes(id)
  \/ \E id \in Artifacts : StageRow(id)
  \/ \E id \in Artifacts : \E who \in Owners : PublishWithReference(id, who)
  \/ \E id \in Artifacts : \E who \in Owners : AddReference(id, who)
  \/ \E id \in Artifacts : \E who \in Owners : PublishWithoutBytes(id, who)
  \/ \E id \in Artifacts : GhostDelete(id)
  \/ \E id \in Artifacts : GcClaim(id)
  \/ \E id \in Artifacts : GcDelete(id)
  \/ \E id \in Artifacts : Abandon(id)
  \/ \E id \in Artifacts : DropOwner(id)
  \/ Stutter   \* the system is open-ended: an idle step is always possible

Init ==
  /\ row = [ id \in Artifacts |-> "none" ]
  /\ disk = [ id \in Artifacts |-> "absent" ]
  /\ holders = [ id \in Artifacts |-> {} ]
  /\ owner = [ id \in Artifacts |-> FALSE ]
  /\ staged = [ id \in Artifacts |-> FALSE ]
  /\ released = {}

Spec == Init /\ [][Next]_vars

\* ---------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A id \in Artifacts : row[id] \in States
  /\ \A id \in Artifacts : disk[id] \in Disk
  /\ \A id \in Artifacts : holders[id] \subseteq Owners
  /\ released \subseteq Artifacts

\* §4.3: a database reference never points at bytes that were not persisted
NoReferenceToUnpersisted ==
  \A id \in Artifacts : holders[id] # {} => disk[id] = "present"

\* a LIVE artifact always has its bytes
LiveIsPersisted ==
  \A id \in Artifacts : row[id] = "LIVE" => disk[id] = "present"

\* GC only ever claims artifacts nobody references and no owner protects
GcClaimsOnlyUnreferencedLive ==
  \A id \in Artifacts : row[id] = "DELETING" => holders[id] = {}

\* D-436: a hold is released only when its owner has finished **and** the artifact is past the retention window
\* (an artifact is ordinary history in §9's words, and the window is the same one that sweeps the events
\* referencing it). The two counterfactuals refute one half each: `ReleaseLiveOwner` releases a live owner's
\* hold, `ReleaseFreshArtifact` releases one inside the window.
OwnerHoldsUntilItsSubjectIsFinished == released \subseteq Spent
NothingIsReleasedInsideTheWindow == released \subseteq Aged

\* and the release is not a deletion: it changes the hold, never the row or the bytes
ReleaseKeepsTheRowAndTheBytes ==
  \A id \in Artifacts : id \in released => disk[id] = "present" \/ row[id] = "none"

\* STAGING/ABANDONED artifacts are never deleted by the collector
CollectorSkipsIncomplete ==
  \A id \in Artifacts : disk[id] = "absent" => row[id] = "none"

\* properties (the interesting parts are action properties) ----------------
\* A30: a reference attaches either to an already LIVE artifact or in the very
\* step that flips STAGING -> LIVE (the first owner publishes by referencing);
\* in both cases the bytes are already persisted
ReferencesOnlyLive ==
  [][ \A id \in Artifacts :
        (holders'[id] # holders[id]) =>
          (row[id] = "LIVE" \/ row'[id] = "LIVE") /\ disk[id] = "present" ]_vars

\* A30: the file never disappears unless the artifact was in DELETING
BytesOnlyDeletedWhileDeleting ==
  [][ \A id \in Artifacts :
        (disk'[id] = "absent" /\ disk[id] = "present") => row[id] = "DELETING" ]_vars

\* A30: GC only claims LIVE artifacts (never STAGING/ABANDONED/DELETING)
ClaimOnlyFromLive ==
  [][ \A id \in Artifacts :
        (row'[id] = "DELETING" /\ row[id] # "DELETING") => row[id] = "LIVE" ]_vars

\* §4.3: the LIVE flip and the first reference are one atomic step
LiveFlipCarriesReference ==
  [][ \A id \in Artifacts :
        (row'[id] = "LIVE" /\ row[id] # "LIVE") => holders'[id] # {} ]_vars

=============================================================================
