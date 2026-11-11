------------------------------- MODULE V2Review ------------------------------
(***************************************************************************
D-503: the bounded automatic-approval review lifecycle.

Code anchors: core/src/v2/control.rs::{begin_review,record_review_attempt,
review_decision}, engine/src/v2/{driver.rs,reviewer.rs}. One parked operation
has at most one live review. A completed consistent review may allow a shell
operation; self-update remains user-only. Review calls do not advance the
parked instance. Every priced response contributes known usage, including a
malformed FAILED response; an unpriced completion and transport/interruption
usage increment the unknown counter. Closing releases reservations. Beginning
from COMPLETE covers a replacement driver after the record/decision crash
window: the old review is already settled and the new request is not a replay.

Abstraction: one operation, integer token totals and explicit assessment
consistency stand in for SQL rows, provider payloads and strict JSON parsing.
Only the latest review's status is retained. Parser quality, real-model
judgement, prices, provider fair scheduling and Rust/SQLite refinement are
outside this model. Bad selects one missing guard; each behavioral claim has
its own refuting configuration.
***************************************************************************)
EXTENDS Naturals
CONSTANT Bad
VARIABLES
  approval, operation, intent, review, consistent, phase,
  reserved, known, pricedResponses, unknown, effect
vars == <<approval, operation, intent, review, consistent, phase,
          reserved, known, pricedResponses, unknown, effect>>

ReviewClosed == {"COMPLETE", "FAILED", "CANCELLED"}

BeginReview ==
  /\ approval = "PENDING" /\ operation = "PREPARED"
  /\ review \in {"NONE", "COMPLETE", "FAILED", "CANCELLED"}
  /\ review' = "PENDING"
  /\ consistent' = FALSE
  /\ reserved' = TRUE
  /\ UNCHANGED <<approval, operation, intent, phase, known, pricedResponses, unknown, effect>>

RecordComplete(valid, priced) ==
  /\ review = "PENDING" /\ valid \in BOOLEAN /\ priced \in BOOLEAN
  /\ priced => known <= 15 /\ pricedResponses < 4
  /\ ~priced => unknown < 3
  /\ review' = "COMPLETE"
  /\ consistent' = valid
  /\ known' = IF priced THEN known + 5 ELSE known
  /\ pricedResponses' = pricedResponses + IF priced THEN 1 ELSE 0
  /\ unknown' = unknown + IF priced THEN 0 ELSE 1
  /\ reserved' = IF Bad = "keep_reservation" THEN TRUE ELSE FALSE
  /\ phase' = IF Bad = "advance_phase" THEN "READY" ELSE phase
  /\ UNCHANGED <<approval, operation, intent, effect>>

RecordMalformedKnown ==
  /\ review = "PENDING" /\ known <= 15 /\ pricedResponses < 4
  /\ review' = "FAILED"
  /\ consistent' = FALSE
  /\ known' = IF Bad = "drop_known_usage" THEN known ELSE known + 5
  /\ pricedResponses' = pricedResponses + 1
  /\ reserved' = IF Bad = "keep_reservation" THEN TRUE ELSE FALSE
  /\ UNCHANGED <<approval, operation, intent, phase, unknown, effect>>

FailUnknown ==
  /\ review = "PENDING" /\ unknown < 3
  /\ review' = "FAILED"
  /\ consistent' = FALSE
  /\ unknown' = unknown + 1
  /\ reserved' = IF Bad = "keep_reservation" THEN TRUE ELSE FALSE
  /\ UNCHANGED <<approval, operation, intent, phase, known, pricedResponses, effect>>

Restart ==
  /\ review = "PENDING" /\ unknown < 3
  /\ review' = "CANCELLED"
  /\ consistent' = FALSE
  /\ reserved' = FALSE
  /\ unknown' = unknown + 1
  /\ UNCHANGED <<approval, operation, intent, phase, known, pricedResponses, effect>>

ReviewAllow ==
  /\ approval = "PENDING" /\ operation = "PREPARED"
  /\ (review = "COMPLETE" \/ Bad = "unreviewed")
  /\ (consistent \/ Bad = "inconsistent")
  /\ (intent = "shell" \/ Bad = "self_update")
  /\ approval' = "APPROVED"
  /\ UNCHANGED <<operation, intent, review, consistent, phase,
                 reserved, known, pricedResponses, unknown, effect>>

ReviewDeny ==
  /\ approval = "PENDING" /\ review # "PENDING"
  /\ approval' = "DENIED"
  /\ operation' = "CANCELLED"
  /\ phase' = "CLOSED"
  /\ reserved' = FALSE
  /\ UNCHANGED <<intent, review, consistent, known, pricedResponses, unknown, effect>>

Dispatch ==
  /\ approval = "APPROVED" /\ operation = "PREPARED"
  /\ operation' = "CLOSED"
  /\ phase' = "CLOSED"
  /\ effect' = TRUE
  /\ UNCHANGED <<approval, intent, review, consistent,
                 reserved, known, pricedResponses, unknown>>

Stutter == UNCHANGED vars
Next == BeginReview
        \/ \E valid \in BOOLEAN: \E priced \in BOOLEAN: RecordComplete(valid, priced)
        \/ RecordMalformedKnown \/ FailUnknown \/ Restart
        \/ ReviewAllow \/ ReviewDeny \/ Dispatch \/ Stutter

Init ==
  /\ approval = "PENDING" /\ operation = "PREPARED"
  /\ intent \in {"shell", "request_self_update"}
  /\ review = "NONE" /\ consistent = TRUE /\ phase = "PARKED"
  /\ reserved = FALSE /\ known = 0 /\ pricedResponses = 0 /\ unknown = 0
  /\ effect = FALSE

Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ approval \in {"PENDING", "APPROVED", "DENIED"}
  /\ operation \in {"PREPARED", "CANCELLED", "CLOSED"}
  /\ intent \in {"shell", "request_self_update"}
  /\ review \in {"NONE", "PENDING", "COMPLETE", "FAILED", "CANCELLED"}
  /\ consistent \in BOOLEAN /\ phase \in {"PARKED", "READY", "CLOSED"}
  /\ reserved \in BOOLEAN /\ known \in 0..20 /\ pricedResponses \in 0..4 /\ unknown \in 0..3
  /\ effect \in BOOLEAN

AllowRequiresCompletedReview == approval = "APPROVED" => review = "COMPLETE"
AllowRequiresConsistentAssessment == approval = "APPROVED" => consistent
SelfUpdateRemainsUserOnly == approval = "APPROVED" => intent = "shell"
ReviewLeavesInstanceParked ==
  operation = "PREPARED" /\ review \in {"PENDING", "COMPLETE", "FAILED", "CANCELLED"} => phase = "PARKED"
ClosedReviewReleasesReservation == review \in ReviewClosed => ~reserved
FailedKnownUsageIsSettled == known = 5 * pricedResponses
=============================================================================
