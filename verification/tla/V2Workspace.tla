------------------------------ MODULE V2Workspace -----------------------------
(***************************************************************************)
(* One member's workspace lifecycle (§5.1/§12.3, D-46/D-76, A12's sibling):  *)
(* a `git_worktree` member works in its own checkout and on its own branch,   *)
(* and two operations move between that member and the session: the merge     *)
(* (`teamagents instances merge --id ID`, D-252) and retirement (§5.4, D-88). *)
(* The rules the code and the documents state, and which this module makes    *)
(* refutable:                                                                *)
(*                                                                           *)
(*   * **uncommitted work is never merged**: the merge takes the *branch*, so *)
(*     changes still sitting in the checkout would be left behind — the       *)
(*     workspace module's rule is that nothing in a member's directory is     *)
(*     dropped silently (`UncommittedWorkIsNeverMerged`);                     *)
(*   * **a member is not merged while its turn runs**: a turn in flight is    *)
(*     writing the very files the merge would bring in, so the merge waits for *)
(*     an idle member (`NoMergeWhileATurnRuns`);                              *)
(*   * **retirement never buries work**: the checkout goes only when nothing   *)
(*     uncommitted and nothing unmerged is in it, which is what the documented *)
(*     refusal "a directory with uncommitted or unmerged work is never        *)
(*     deleted, only reported" means (`RetirementNeverBuriesWork`).           *)
(*                                                                           *)
(* Code anchors: engine/src/v2/intervene.rs (`merge_member`: the member's      *)
(* record supplies the branch, the checkout is asked whether it is dirty, the  *)
(* merge is `workspace::merge_branch`, and conflicts are reported rather than  *)
(* decided); engine/src/workspace.rs (`worktree_is_dirty` / `dirty_status`,    *)
(* `branch_exists`, `cleanup`/`check_worktree_cleanup` for the retire half);   *)
(* engine/src/v2/driver.rs (`prepare_spawn_workspace` writes the record before *)
(* the instance boots, so a later retirement cleans up exactly what it made).  *)
(*                                                                           *)
(* The live halves are review/dogfood/workspace.py (a real model walks the      *)
(* lifecycle, and since D-252 merges through the product's own lever) and       *)
(* engine/tests/v2_daemon.rs's merge test (the real binary, a real git         *)
(* repository, a real daemon).                                                 *)
(*                                                                           *)
(* What the model is and is not: it abstracts the *branch* into `unmerged`     *)
(* ("the branch carries commits the session tree does not") and the checkout   *)
(* into `uncommitted` + `checkout`; it does not model git's own conflict state *)
(* (the merge either lands or is refused — a conflict is the third answer, and *)
(* the verb reports it with git's message instead of deciding), the worktree's *)
(* path or records, the isolated/shared policies, or the supervisor's          *)
(* discovery cadence. Retirement is the supervisor's step and the merge is the *)
(* user's.                                                                     *)
(*                                                                           *)
(* The two counterfactuals are the two mistakes those rules exist against:     *)
(* `MergeWithoutChecking` (the merge that ignores a dirty checkout or a        *)
(* running turn — what a bare `git merge` driven by a listing tool does) and   *)
(* `RetireBuryingWork` (the retirement that deletes the checkout anyway, the   *)
(* pre-D-76 shape whose silence is why the user never learned what was lost).  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS MergeWithoutChecking,   \* counterfactual: the merge ignores the dirty/busy guards
          RetireBuryingWork        \* counterfactual: retirement deletes a checkout that still holds work

VARIABLES
  checkout,     \* the member's worktree: none | present
  uncommitted,  \* the checkout holds changes that are not in its branch
  unmerged,     \* the branch holds commits the session's tree does not
  busy,         \* the member has a turn in flight (it is writing files right now)
  mergedDirty,  \* monitor: a merge landed while the checkout was dirty
  mergedBusy,   \* monitor: a merge landed while a turn was in flight
  buried        \* monitor: a retirement took a checkout that still held work

vars == <<checkout, uncommitted, unmerged, busy>>
monVars == <<vars, mergedDirty, mergedBusy, buried>>

\* ------------------------------------------------------------------- actions --
\* The spawn resolves the policy and creates the checkout (D-76).
Prepare ==
  /\ checkout = "none"
  /\ checkout' = "present"
  /\ UNCHANGED <<uncommitted, unmerged, busy, mergedDirty, mergedBusy, buried>>

\* The member writes in its own tree: the change lives in the checkout until it is committed.
MemberEdits ==
  /\ checkout = "present"
  /\ uncommitted' = TRUE
  /\ UNCHANGED <<checkout, unmerged, busy, mergedDirty, mergedBusy, buried>>

\* The member commits: the work moves from the checkout into the branch.
MemberCommits ==
  /\ checkout = "present"
  /\ uncommitted
  /\ uncommitted' = FALSE
  /\ unmerged' = TRUE
  /\ UNCHANGED <<checkout, busy, mergedDirty, mergedBusy, buried>>

\* A turn starts and ends (the driver's phase machine, abstracted to one bit).
TurnStarts ==
  /\ ~busy
  /\ busy' = TRUE
  /\ UNCHANGED <<checkout, uncommitted, unmerged, mergedDirty, mergedBusy, buried>>

TurnEnds ==
  /\ busy
  /\ busy' = FALSE
  /\ UNCHANGED <<checkout, uncommitted, unmerged, mergedDirty, mergedBusy, buried>>

\* D-252: the merge (`instances merge --id ID`). It carries the *branch* into the session's tree, so it refuses
\* while the checkout holds work the branch does not, and while the member is writing (a turn in flight). A
\* retired member's branch survives its checkout (retirement refuses to delete unmerged work), which is why this
\* action does not require the checkout to exist.
Merge ==
  /\ (~uncommitted \/ MergeWithoutChecking)
  /\ (~busy \/ MergeWithoutChecking)
  /\ unmerged' = FALSE
  /\ mergedDirty' = (mergedDirty \/ (uncommitted /\ MergeWithoutChecking))
  /\ mergedBusy' = (mergedBusy \/ (busy /\ MergeWithoutChecking))
  /\ UNCHANGED <<checkout, uncommitted, busy, buried>>

\* §5.4/D-76: retirement removes the checkout — but only when nothing would be lost ("a directory with
\* uncommitted or unmerged work is never deleted, only reported").
Retire ==
  /\ checkout = "present"
  /\ ((~uncommitted /\ ~unmerged) \/ RetireBuryingWork)
  /\ checkout' = "none"
  /\ buried' = (buried \/ ((uncommitted \/ unmerged) /\ RetireBuryingWork))
  /\ UNCHANGED <<uncommitted, unmerged, busy, mergedDirty, mergedBusy>>

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED monVars

Next ==
  \/ Prepare
  \/ MemberEdits
  \/ MemberCommits
  \/ TurnStarts
  \/ TurnEnds
  \/ Merge
  \/ Retire
  \/ Stutter

Init ==
  /\ checkout = "none"
  /\ uncommitted = FALSE
  /\ unmerged = FALSE
  /\ busy = FALSE
  /\ mergedDirty = FALSE
  /\ mergedBusy = FALSE
  /\ buried = FALSE

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ checkout \in {"none", "present"}
  /\ uncommitted \in BOOLEAN
  /\ unmerged \in BOOLEAN
  /\ busy \in BOOLEAN
  /\ mergedDirty \in BOOLEAN
  /\ mergedBusy \in BOOLEAN
  /\ buried \in BOOLEAN

\* D-252: the merge never takes the branch while work is still sitting in the checkout — that work would be
\* invisible to the merge, which is the "nothing in a member's directory is dropped silently" rule.
UncommittedWorkIsNeverMerged == mergedDirty = FALSE

\* ... and it never lands while the member's turn is in flight: a running member is writing the files the merge
\* would bring in.
NoMergeWhileATurnRuns == mergedBusy = FALSE

\* D-76: a checkout that still holds work (uncommitted or unmerged) is never retired, only reported.
RetirementNeverBuriesWork == buried = FALSE

=============================================================================
