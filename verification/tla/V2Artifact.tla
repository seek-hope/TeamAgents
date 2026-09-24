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
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Artifacts,  \* artifact slots, e.g. {"a1","a2"}
          Owners      \* reference owners, e.g. {"r1","r2"}

ASSUME Artifacts # {} /\ Owners # {}

States == {"none", "STAGING", "LIVE", "DELETING", "ABANDONED"}
Disk == {"absent", "present"}

VARIABLES
  row,      \* artifact -> lifecycle state in the database
  disk,     \* artifact -> whether the bytes are durably on disk
  holders,  \* artifact -> set of live reference owners (bounded on purpose)
  owner,    \* artifact -> whether a publishing owner is still protecting it
  staged    \* artifact -> the tmp file was written but the row may not exist

vars == <<row, disk, holders, owner, staged>>

\* ------------------------------------------------------------------- actions --
\* 1) bytes first: tmp file, fsync, rename (a crash here leaves an orphan file)
WriteBytes(id) ==
  /\ disk[id] = "absent"
  /\ staged[id] = FALSE
  /\ disk' = [disk EXCEPT ![id] = "present"]
  /\ staged' = [staged EXCEPT ![id] = TRUE]
  /\ UNCHANGED <<row, holders, owner>>

\* 2) then the STAGING row with its owner (crash between 1 and 2 = orphan file)
StageRow(id) ==
  /\ staged[id] /\ row[id] = "none"
  /\ row' = [row EXCEPT ![id] = "STAGING"]
  /\ owner' = [owner EXCEPT ![id] = TRUE]
  /\ UNCHANGED <<disk, holders, staged>>

\* 3) the referencing transaction: LIVE flip and the reference commit together
PublishWithReference(id, who) ==
  /\ row[id] = "STAGING" /\ disk[id] = "present"
  /\ who \in Owners \ holders[id]
  /\ row' = [row EXCEPT ![id] = "LIVE"]
  /\ holders' = [holders EXCEPT ![id] = @ \cup {who}]
  /\ owner' = [owner EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<disk, staged>>

\* add another reference to an already live artifact (new owner attaches)
AddReference(id, who) ==
  /\ row[id] = "LIVE" /\ disk[id] = "present"
  /\ who \in Owners \ holders[id]
  /\ holders' = [holders EXCEPT ![id] = @ \cup {who}]
  /\ UNCHANGED <<row, disk, owner, staged>>

\* GC claim: only ownerless, unreferenced LIVE artifacts become DELETING
GcClaim(id) ==
  /\ row[id] = "LIVE" /\ holders[id] = {} /\ ~owner[id]
  /\ row' = [row EXCEPT ![id] = "DELETING"]
  /\ UNCHANGED <<disk, holders, owner, staged>>

\* GC delete: the file goes away, the row leaves the catalog
GcDelete(id) ==
  /\ row[id] = "DELETING" /\ holders[id] = {}
  /\ disk' = [disk EXCEPT ![id] = "absent"]
  /\ row' = [row EXCEPT ![id] = "none"]
  /\ staged' = [staged EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<holders, owner>>

\* orphan cleanup: a STAGING artifact whose publishing request died is abandoned
Abandon(id) ==
  /\ row[id] = "STAGING"
  /\ row' = [row EXCEPT ![id] = "ABANDONED"]
  /\ owner' = [owner EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<disk, holders, staged>>

\* a dropped owner stops protecting an aborted publish (still not claimable
\* while the row is STAGING: GC only claims LIVE)
DropOwner(id) ==
  /\ owner[id]
  /\ owner' = [owner EXCEPT ![id] = FALSE]
  /\ UNCHANGED <<row, disk, holders, staged>>

\* ---------------------------------------------------------------------- spec --
\* an idle step keeps the model open-ended (TLC then reports no false deadlock)
Stutter == UNCHANGED vars

Next ==
  \/ \E id \in Artifacts : WriteBytes(id)
  \/ \E id \in Artifacts : StageRow(id)
  \/ \E id \in Artifacts : \E who \in Owners : PublishWithReference(id, who)
  \/ \E id \in Artifacts : \E who \in Owners : AddReference(id, who)
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

Spec == Init /\ [][Next]_vars

\* ---------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A id \in Artifacts : row[id] \in States
  /\ \A id \in Artifacts : disk[id] \in Disk
  /\ \A id \in Artifacts : holders[id] \subseteq Owners

\* §4.3: a database reference never points at bytes that were not persisted
NoReferenceToUnpersisted ==
  \A id \in Artifacts : holders[id] # {} => disk[id] = "present"

\* a LIVE artifact always has its bytes
LiveIsPersisted ==
  \A id \in Artifacts : row[id] = "LIVE" => disk[id] = "present"

\* GC only ever claims artifacts nobody references and no owner protects
GcClaimsOnlyUnreferencedLive ==
  \A id \in Artifacts : row[id] = "DELETING" => holders[id] = {}

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
