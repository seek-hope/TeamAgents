------------------------------ MODULE V2Prune -------------------------------
(***************************************************************************)
(* What `sessions prune` may take, and what it must leave alone (D-435).       *)
(*                                                                            *)
(* `[retention] archived_days` was accepted and never applied since D-245 —    *)
(* one state root meant one session, so there was no archived set to walk.     *)
(* Named sessions (D-364) gave the key its meaning: `teamagents sessions prune` *)
(* removes an *archived* session whose archive time is older than the window.  *)
(* The rules the code and the documents state are:                            *)
(*                                                                            *)
(*   * only **archived** sessions are candidates — the state root a user works *)
(*     in is the base itself, which is not a registry row, so it cannot be     *)
(*     chosen. That is an encoding claim rather than an invariant: `Sessions`  *)
(*     holds archived rows only, and no action can name anything else;          *)
(*     `TypeOK` states it, and the docstring is where a reader checks it;       *)
(*   * a row with **no recorded archive time** is kept — the rule deletes what *)
(*     it can date, never a state root on a guess (`NothingUndeletedLeaves`);  *)
(*   * a session a **daemon still serves** is kept, with the lever to stop it  *)
(*     (`NothingServedLeaves`);                                               *)
(*   * the **`EVIDENCE` marker** wins over the window (`NothingMarkedLeaves`); *)
(*   * and the **window** is a window: nothing inside it goes                  *)
(*     (`NothingInsideTheWindowLeaves`).                                      *)
(*                                                                            *)
(* Code anchors: engine/src/v2/sessions.rs — `Registry::prune` (the four       *)
(* guards, in that order, and the report that names a reason per kept row),    *)
(* `is_live` (the same liveness the archive and delete verbs refuse on), and   *)
(* `directory_bytes` (what the report counts); `core/src/models.rs`'s          *)
(* `Retention::archived_days` (the field the CLI reads, 0 meaning "keep");     *)
(* `engine/src/main.rs`'s `sessions prune` arm (the window from the config or  *)
(* `--days N`, the `--yes`/`--dry-run` contract, `--days 0` refused).          *)
(*                                                                            *)
(* What the model is and is not: one prune over an abstract set of archived    *)
(* rows, and the *selection* only. Whether the bytes really leave the disk,    *)
(* what a kept row's reason says, and the clock are outside it — the live half *)
(* is `review/dogfood/sessions_prune.py` (it backdates an archive by rewriting *)
(* `archived_ms`, which is the file the verb reads, and checks the directory   *)
(* and the registry record afterwards) and the deterministic halves are        *)
(* `sessions::tests::prune_takes_only_archived_sessions_past_the_window_and_   *)
(* says_why_the_rest_stay`, `...::a_dry_run_reports_the_same_set_and_deletes_  *)
(* nothing` and `cli::tests::sessions_prune_needs_a_window_and_a_confirmation_ *)
(* and_its_dry_run_deletes_nothing`. Safety only (nothing that should stay     *)
(* leaves); a prune that takes nothing is not a liveness defect, so there is   *)
(* no fairness assumption.                                                    *)
(*                                                                            *)
(* Four counterfactuals, one per guard, each refuted by the same property:     *)
(* `IgnoreWindow` (age is not consulted), `IgnoreDating` (an undated row is    *)
(* taken), `IgnoreLive` (a served session is taken) and `IgnoreEvidence` (a    *)
(* marked root is taken). Each changes the *action*; the invariant states the  *)
(* intended rule independently, so a control can only be refuted by the rule   *)
(* it breaks.                                                                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Sessions,  \* the *archived* sessions a prune may consider (a registry's rows, archived)
          WindowPassed,  \* Sessions: those archived longer ago than the configured window
          Dated,  \* Sessions: those carrying a recorded archive time
          Served,  \* Sessions: those a daemon is still answering
          Marked,  \* Sessions: those carrying the EVIDENCE marker
          IgnoreWindow,  \* counterfactual: age is not consulted
          IgnoreDating,  \* counterfactual: an undated row is taken
          IgnoreLive,  \* counterfactual: a served session is taken
          IgnoreEvidence,  \* counterfactual: a marked root is taken
          KeepPresent
ASSUME Sessions # {}
ASSUME WindowPassed \subseteq Sessions
ASSUME Dated \subseteq Sessions
ASSUME Served \subseteq Sessions
ASSUME Marked \subseteq Sessions

\* The rule the code follows: an archived session that is dated, past the window, unserved and unmarked.
MayBeTaken(s) ==
  /\ s \in WindowPassed
  /\ s \in Dated
  /\ s \notin Served
  /\ s \notin Marked

\* What the implementation under test may take — the same rule with any guard switched off.
MayBeTakenByDecision(s) ==
  /\ (IgnoreWindow \/ s \in WindowPassed)
  /\ (IgnoreDating \/ s \in Dated)
  /\ (IgnoreLive \/ s \notin Served)
  /\ (IgnoreEvidence \/ s \notin Marked)

VARIABLES
  present,   \* the archived sessions still on disk
  taken,     \* the ones this prune removed
  badlyTaken \* monitor: removed although the intended rule would have kept them — must stay empty

vars == <<present, taken, badlyTaken>>

\* ------------------------------------------------------------------- actions --
\* The user runs the verb, one session at a time (a prune takes a set; the set is the iterative closure of this
\* step, which keeps the state space a clean product and makes each guard's counterexample one step long). The
\* monitor records a session the intended rule would have kept.
PruneOne(s) ==
  /\ s \in present
  /\ MayBeTakenByDecision(s)
  /\ present' = IF KeepPresent THEN present ELSE present \ {s}
  /\ taken' = taken \cup {s}
  /\ badlyTaken' = badlyTaken \cup (IF MayBeTaken(s) THEN {} ELSE {s})

Stutter == UNCHANGED vars

Next == (\E s \in Sessions : PruneOne(s)) \/ Stutter

Init ==
  /\ present = Sessions
  /\ taken = {}
  /\ badlyTaken = {}

Spec == Init /\ [][Next]_vars

\* ---------------------------------------------------------------- invariants --
TypeOK ==
  /\ present \subseteq Sessions
  /\ taken \subseteq Sessions
  /\ badlyTaken \subseteq Sessions

\* The safety property, in one statement: nothing the intended rule keeps is ever taken. Each counterfactual
\* refutes it through the guard it removes.
NothingThatShouldStayLeaves == badlyTaken = {}

\* A session this prune took is gone: the verb has no undo (putting one back is `sessions restore`, a different
\* verb, and only for a session that was not pruned).
TakenIsGone == taken \cap present = {}

=============================================================================
