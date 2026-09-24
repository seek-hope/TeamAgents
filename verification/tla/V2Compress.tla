------------------------------ MODULE V2Compress -----------------------------
(***************************************************************************)
(* Context compression (plan §7/A20; R22 fix): a summary is appended, the    *)
(* entries it covers are hidden — never deleted — and the compression request*)
(* closes in the same transaction. Failure leaves the context untouched.     *)
(*                                                                          *)
(* Code anchors: core/src/v2/control.rs — begin_compression (lifecycle gate   *)
(* plus the shared goal deadline/budget gate, same as a turn request: that     *)
(* gate itself is verified in V2Control.tla), compress_context (append summary *)
(* → mark covered → close request → release reservation, ONE transaction),     *)
(* fail_compression (close + release, context untouched), close_epoch_execution*)
(* (a reset/terminate cancels the pending compression and releases it too).   *)
(*                                                                          *)
(* Entries occupy a prefix of the slot order (append discipline), so "a        *)
(* summary is newer than what it covers" holds by construction; the module      *)
(* states it as an invariant together with "coverage only grows" and "an entry  *)
(* is never lost".                                                             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Slots,   \* entry slots in index order, non-negative, e.g. {0,1,2,3}
          Reqs     \* compression request slots, e.g. {"r1"}

ASSUME Slots # {} /\ Reqs # {}

EntryState == {"none", "live", "covered"}
RequestState == {"none", "PENDING", "COMPLETE", "FAILED", "CANCELLED"}
\* sentinel: the entry is not covered by a summary. It is an integer outside the
\* slot range so that `Slots \cup {nocover}` stays a homogeneous set for TLC.
nocover == 0 - 1

VARIABLES
  entry,        \* slot -> none | live | covered
  isSummary,    \* slot -> whether a commit created it
  summaryOf,    \* slot -> the summary that covers it (nocover = visible)
  requestStatus,\* compression request -> status
  reserved,     \* request -> whether its budget reservation is still held
  lost,         \* monitor: slots that lost their entry (must stay {})
  uncovered     \* monitor: slots that became visible again (must stay {})

vars == <<entry, isSummary, summaryOf, requestStatus, reserved>>
monVars == <<vars, lost, uncovered>>

\* ---------------------------------------------------------- views and guards --
\* the model-visible view: entries that exist and are not covered
Visible(s) == entry[s] = "live" /\ summaryOf[s] = nocover

\* entries occupy a prefix of the slot order (append discipline)
AppendDiscipline == \A s \in Slots : entry[s] = "none" => \A t \in Slots : t > s => entry[t] = "none"

\* the lowest free slot: where the next append lands
NextFree == CHOOSE s \in Slots : entry[s] = "none" /\ \A t \in Slots : t < s => entry[t] # "none"

\* ------------------------------------------------------------------- actions --
\* an ordinary context entry lands at the tail
AppendEntry ==
  /\ \E s \in Slots : entry[s] = "none"
  /\ LET s == NextFree IN
     /\ entry' = [entry EXCEPT ![s] = "live"]
     /\ isSummary' = [isSummary EXCEPT ![s] = FALSE]
     /\ lost' = lost
     /\ uncovered' = uncovered
  /\ UNCHANGED <<summaryOf, requestStatus, reserved>>

\* begin_compression: a compression request opens, reserving budget. The
\* deadline/budget gate itself is V2Control's AdmissionGate (same code path).
BeginCompression(r) ==
  /\ requestStatus[r] = "none"
  /\ requestStatus' = [requestStatus EXCEPT ![r] = "PENDING"]
  /\ reserved' = [reserved EXCEPT ![r] = TRUE]
  /\ lost' = lost
  /\ uncovered' = uncovered
  /\ UNCHANGED <<entry, isSummary, summaryOf>>

\* compress_context: append the summary at the tail, hide every visible entry
\* that the caller did not keep, close the request and release the reservation —
\* one transaction. `summaryOf` is only ever written here, and only for entries
\* that are visible right now, so coverage can never be lifted or re-pointed.
CommitCompression(r, keep) ==
  /\ requestStatus[r] = "PENDING"
  /\ \E s \in Slots : entry[s] = "none"
  /\ keep \subseteq Slots
  /\ LET s == NextFree
         covered == { e \in Slots : Visible(e) /\ e < s /\ e \notin keep } IN
     /\ entry' = [ e \in Slots |-> IF e = s THEN "live"
                                ELSE IF e \in covered THEN "covered"
                                ELSE entry[e] ]
     /\ isSummary' = [isSummary EXCEPT ![s] = TRUE]
     /\ summaryOf' = [ e \in Slots |-> IF e \in covered THEN s ELSE summaryOf[e] ]
     /\ requestStatus' = [requestStatus EXCEPT ![r] = "COMPLETE"]
     /\ reserved' = [reserved EXCEPT ![r] = FALSE]
     /\ lost' = lost
     /\ uncovered' = uncovered

\* fail_compression: the summary never arrived; the request closes and its
\* reservation releases, the context is untouched (a lost optimization).
FailCompression(r) ==
  /\ requestStatus[r] = "PENDING"
  /\ requestStatus' = [requestStatus EXCEPT ![r] = "FAILED"]
  /\ reserved' = [reserved EXCEPT ![r] = FALSE]
  /\ lost' = lost
  /\ uncovered' = uncovered
  /\ UNCHANGED <<entry, isSummary, summaryOf>>

\* a reset or termination closes the epoch: the pending compression asks are
\* cancelled and released (close_epoch_execution), still without touching the
\* context of the epoch it is sealing.
CancelCompression(r) ==
  /\ requestStatus[r] = "PENDING"
  /\ requestStatus' = [requestStatus EXCEPT ![r] = "CANCELLED"]
  /\ reserved' = [reserved EXCEPT ![r] = FALSE]
  /\ lost' = lost
  /\ uncovered' = uncovered
  /\ UNCHANGED <<entry, isSummary, summaryOf>>

Stutter == UNCHANGED monVars

Next ==
  \/ AppendEntry
  \/ \E r \in Reqs : BeginCompression(r)
  \/ \E r \in Reqs : \E keep \in SUBSET Slots : CommitCompression(r, keep)
  \/ \E r \in Reqs : FailCompression(r)
  \/ \E r \in Reqs : CancelCompression(r)
  \/ Stutter

Init ==
  /\ entry = [ s \in Slots |-> "none" ]
  /\ isSummary = [ s \in Slots |-> FALSE ]
  /\ summaryOf = [ s \in Slots |-> nocover ]
  /\ requestStatus = [ r \in Reqs |-> "none" ]
  /\ reserved = [ r \in Reqs |-> FALSE ]
  /\ lost = {}
  /\ uncovered = {}

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A s \in Slots : entry[s] \in EntryState
  /\ \A s \in Slots : summaryOf[s] \in Slots \cup {nocover}
  /\ \A r \in Reqs : requestStatus[r] \in RequestState
  /\ \A r \in Reqs : reserved[r] \in BOOLEAN

\* entries fill a prefix: appends land at the tail, nothing is inserted in place
TailAppend == AppendDiscipline

\* A20/§4.3: coverage is a view fact, never a deletion — an entry that ever
\* existed still exists (monitored: `lost` stays empty)
NoEntryIsEverLost == lost = {}

\* a summary is always newer than what it covers
CoveragePointsForward ==
  \A s \in Slots : summaryOf[s] # nocover => s < summaryOf[s]

\* coverage is never lifted or re-pointed at another summary (monitored)
CoverageNeverLifted == uncovered = {}

\* the summary a commit just appended is visible: the newest summary is never
\* itself covered (older summaries may be, by a later one)
NewestSummaryIsVisible ==
  \A s \in Slots : isSummary[s] /\ entry[s] # "none" /\ (\A t \in Slots : isSummary[t] /\ entry[t] # "none" => t <= s)
                    => summaryOf[s] = nocover

\* a summary covers only entries that existed at commit time and were visible
CoveredStaysCoveredByItsSummary ==
  \A s \in Slots : entry[s] = "covered" => isSummary[summaryOf[s]] /\ summaryOf[s] > s

\* every closed compression request released its reservation (complete, failed
\* or cancelled by an epoch close)
ClosedCompressionReleasesReservation ==
  \A r \in Reqs : requestStatus[r] # "PENDING" => ~reserved[r]

\* a request only ever closes once: no status is rewritten after it left PENDING
RequestClosesOnce ==
  \A r \in Reqs : requestStatus[r] \in {"COMPLETE", "FAILED", "CANCELLED"}
                  \/ requestStatus[r] \in {"none", "PENDING"}

=============================================================================
