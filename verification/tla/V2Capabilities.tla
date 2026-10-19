------------------------------- MODULE V2Capabilities ------------------------
(***************************************************************************)
(* D-455 capability delegation, kept deliberately small. The broad authority  *)
(* models (V2Grants/V2Authority) keep the grant surface; this module checks    *)
(* the transition properties that the new protocol adds: a user marks a parent *)
(* delegable, the leader mints an attenuated child, task settlement revokes    *)
(* it, and parent revocation cascades.                                        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS Tasks, AllowUndelegableMint

VARIABLES
  parent_grant,
  parent_live,
  parent_delegable,
  child_exists,
  child_live,
  child_task,
  child_delegable,
  child_parent,
  settled

vars == <<parent_grant, parent_live, parent_delegable, child_exists, child_live,
          child_task, child_delegable, child_parent, settled>>

Init ==
  /\ parent_grant = "none"
  /\ parent_live = FALSE
  /\ parent_delegable = FALSE
  /\ child_exists = FALSE
  /\ child_live = FALSE
  /\ child_task = "none"
  /\ child_delegable = FALSE
  /\ child_parent = "none"
  /\ settled = FALSE

\* The user is the only source of a delegable grant.
UserGrants ==
  /\ ~parent_live
  /\ parent_grant' = "g-parent"
  /\ parent_live' = TRUE
  /\ parent_delegable' = TRUE
  /\ UNCHANGED <<child_exists, child_live, child_task, child_delegable, child_parent, settled>>

\* The leader may mint only while the parent is live and delegable; the child
\* is task-scoped and never delegable.
MintChild ==
  /\ parent_live /\ parent_delegable /\ ~child_exists
  /\ child_exists' = TRUE
  /\ child_live' = TRUE
  /\ child_task' = CHOOSE t \in Tasks : TRUE
  /\ child_delegable' = FALSE
  /\ child_parent' = parent_grant
  /\ UNCHANGED <<parent_grant, parent_live, parent_delegable, settled>>

\* Task settlement consumes the child grant.
Settle ==
  /\ child_exists /\ ~settled
  /\ settled' = TRUE
  /\ child_live' = FALSE
  /\ UNCHANGED <<parent_grant, parent_live, parent_delegable, child_exists,
                 child_task, child_delegable, child_parent>>

\* Revoking the parent takes the child with it.
RevokeParent ==
  /\ parent_live
  /\ parent_live' = FALSE
  /\ child_live' = FALSE
  /\ UNCHANGED <<parent_grant, parent_delegable, child_exists, child_task,
                 child_delegable, child_parent, settled>>

\* Negative control: mint a child with no parent at all. The bad config must
\* make NoChildWithoutParent fail; the default config never enables this.
BadMint ==
  /\ ~child_exists
  /\ child_exists' = TRUE
  /\ child_live' = TRUE
  /\ child_task' = CHOOSE t \in Tasks : TRUE
  /\ child_delegable' = FALSE
  /\ child_parent' = "none"
  /\ UNCHANGED <<parent_grant, parent_live, parent_delegable, settled>>

Next ==
  \/ UserGrants
  \/ MintChild
  \/ Settle
  \/ RevokeParent
  \/ (AllowUndelegableMint /\ BadMint)
  \/ UNCHANGED vars

Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------------ invariants --
\* A child never exists without a delegable parent and a named parent grant.
NoChildWithoutParent == child_exists => (child_parent # "none" /\ parent_delegable)
\* A child is never delegable.
ChildNonDelegable == child_exists => ~child_delegable
\* Settlement revokes the child grant.
TaskSettleRevokes == settled => ~child_live
\* Parent revocation cascades to the child.
ParentRevokeCascades == ~parent_live => ~child_live

\* ------------------------------------------------------------------ properties --

=============================================================================
