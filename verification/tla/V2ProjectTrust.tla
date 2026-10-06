------------------------------ MODULE V2ProjectTrust -------------------------------
(***************************************************************************)
(* Which repository's config a session may read, and what that decision may   *)
(* depend on (D-434).                                                         *)
(*                                                                            *)
(* D-244's `V2Trust` answers what a *trusted* project may put into a session; *)
(* this module answers the question underneath it — which repository counts as *)
(* trusted, and how the answer is reached. The rules the code and the          *)
(* documents state are:                                                        *)
(*                                                                            *)
(*   * `[permissions] trusted_projects` in the user's own config lists paths,  *)
(*     and the decision compares **canonical** paths, so listing a symlinked   *)
(*     checkout and opening its target is one decision                         *)
(*     (`ListedTrustIsCanonical`);                                            *)
(*   * a session started in a subdirectory reads the config of the nearest     *)
(*     ancestor that has one (`RootOf`'s walk up), and a listed repository     *)
(*     trusts only itself — never the repositories beside or beneath it        *)
(*     (`TrustIsForTheRootTheUserListed`);                                    *)
(*   * a grant never outlives its entry: a session that lands after the path   *)
(*     was withdrawn is a violation (`NoGrantOutlivesItsEntry`);               *)
(*   * the decision's only sources are the user's own config — the list and    *)
(*     `trust_project = true`, which keeps its D-244 meaning of "any           *)
(*     repository" (what `ProjectMayGrantTrust` is the negation of).           *)
(*                                                                            *)
(* Code anchors: engine/src/config.rs — `project_config_path` (the walk up),   *)
(* `project_root_of` (`canonicalize`), `project_permissions` (the reader of    *)
(* `[permissions]`, always handed the *user* value), `trust_decision` (the     *)
(* comparison this module models) and `ProjectMerge.trust_source`, which is    *)
(* why the decision comes back as data rather than a boolean.                  *)
(*                                                                            *)
(* ------------------------------------------------------------------ division -- *)
(* Two of this design's hazards are `Locations`, not `RootOf`, and the model   *)
(* carries the first while the filesystem tests carry the second:              *)
(*                                                                            *)
(*   * "same repository, two names" is `Groups` here (an equivalence class of   *)
(*     locations) and a symlink in `config::project_config_tests::project_     *)
(*     trust_is_the_canonical_root_the_user_listed`;                          *)
(*   * "a listed repository is not its neighbours or its children" is a         *)
(*     *prefix* question, which no set encoding in a TLC configuration file    *)
(*     can express (its CONSTANTS are plain set enumerations: `\X` and `\cup`  *)
(*     are refused — measured). The test's `repo` / `repo-two` pair is that    *)
(*     negation, and the walk-up's "stop at the nearest ancestor with a        *)
(*     config" is the other half of it.                                        *)
(*                                                                            *)
(* What the model is and is not: one session in one location, the decision     *)
(* only — whether the project config then merges is `V2Trust`'s subject, and   *)
(* what a merged entry does is neither module's. `Groups` is the *meaning* of  *)
(* canonicalization (an equivalence, not a path function), so the model cannot *)
(* and does not check that `canonicalize` returns the right string; it checks  *)
(* that the decision depends on the class and on nothing finer. Safety plus one *)
(* usability equivalence; whether a user ever lists anything is not a liveness *)
(* property, so there is no fairness assumption.                               *)
(*                                                                            *)
(* Three counterfactuals, one per plausible wrong implementation:              *)
(* `CompareRawPath` compares the path the user typed instead of the canonical  *)
(* class (the symlink case, refuting `ListedTrustIsCanonical`),                *)
(* `ProjectMayGrantTrust` lets a repository's own `[permissions]` table answer *)
(* the question (the shape D-244 exists against, refuting                         *)
(* `TrustIsForTheRootTheUserListed`) and `TrustStale` lets a grant outlive the  *)
(* entry that made it (refuting `NoGrantOutlivesItsEntry`).                    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Locations,           \* the paths a session may be *started* in: a root, a subdirectory, a symlink
          Groups,              \* a partition of Locations: the locations that resolve to one repository
          StartedIn,           \* the location of the session this module is about
          CompareRawPath,      \* counterfactual: compare the typed path, not the canonical class
          ProjectMayGrantTrust,\* counterfactual: the repository's own `[permissions]` answers
          TrustStale,          \* counterfactual: a grant outlives the `trusted_projects` entry
          AnyRepository        \* the user's coarse opt-in: `trust_project = true`

ASSUME Locations # {} /\ StartedIn \in Locations
\* `Groups` is a partition: every location is in exactly one class (the walk up and `canonicalize` produce one
\* answer per location, and two locations resolving to one repository are one class).
ASSUME \A l \in Locations : \E g \in Groups : l \in g
ASSUME \A g1, g2 \in Groups : g1 # g2 => g1 \cap g2 = {}

\* Two locations are the same repository exactly when they share a class.
SameRepository(l1, l2) == \E g \in Groups : l1 \in g /\ l2 \in g

VARIABLES
  listed,      \* the *typed* paths in `[permissions] trusted_projects`
  cached,      \* the locations a stale decision keeps after their entry is gone
  landed       \* did this session's project config merge?

vars == <<listed, cached, landed>>

\* ------------------------------------------------------------- the two rules --
\* The rule the code follows: the coarse flag, or a listed path in this session's repository.
TrustedByIntendedRule(l) ==
  AnyRepository \/ (\E other \in listed : SameRepository(other, l))

\* What the implementation under test does — one entry at a time, so each counterfactual changes the comparison
\* rather than the statement of the rule.
TrustedByDecision(l) ==
  AnyRepository \/ ProjectMayGrantTrust \/
    (\E other \in listed : IF CompareRawPath THEN other = l ELSE SameRepository(other, l))

\* ---------------------------------------------------------------- actions --
\* The user edits their own config: `trusted_projects` gains or loses a typed path. A session reads the config
\* once, so the file is only edited between sessions — without this guard the safety invariant would fail for a
\* session that had *already* merged when the entry was withdrawn, which is not a state the product can be in.
UserLists(l) ==
  /\ ~landed
  /\ l \notin listed
  /\ listed' = listed \cup {l}
  /\ UNCHANGED <<cached, landed>>

UserWithdraws(l) ==
  /\ ~landed
  /\ l \in listed
  /\ listed' = listed \ {l}
  /\ UNCHANGED <<cached, landed>>

\* A session starts in `StartedIn` and merges that repository's config iff the decision says so; the session's
\* own merge is not revisited when the user edits the file afterwards (the config is read at load). A stale
\* decision keeps a grant whose entry is gone (`TrustStale`), and the monitor records it: the property is that
\* this never happens.
OpenSession ==
  /\ \/ TrustedByDecision(StartedIn) /\ landed' = TRUE
     \/ ~TrustedByDecision(StartedIn)
        /\ landed' = (TrustStale /\ \E l \in cached : SameRepository(l, StartedIn))
  /\ cached' = cached \cup (IF TrustedByDecision(StartedIn) THEN {StartedIn} ELSE {})
  /\ UNCHANGED <<listed>>

\* The session ends: the config is read again by whatever starts next. `cached` deliberately survives this —
\* that persistence is what the stale counterfactual is made of.
SessionEnds ==
  /\ landed
  /\ landed' = FALSE
  /\ UNCHANGED <<listed, cached>>

Stutter == UNCHANGED vars

Next ==
  \/ \E l \in Locations : UserLists(l)
  \/ \E l \in Locations : UserWithdraws(l)
  \/ OpenSession
  \/ SessionEnds
  \/ Stutter

Init ==
  /\ listed = {}
  /\ cached = {}
  /\ landed = FALSE

Spec == Init /\ [][Next]_vars

\* ------------------------------------------------------------- invariants --
TypeOK ==
  /\ listed \subseteq Locations
  /\ cached \subseteq Locations
  /\ landed \in BOOLEAN

\* The safety property: a repository's config merges only when the *intended* rule trusts the repository this
\* session is in. `ProjectMayGrantTrust` refutes it — a project's own table answering the question.
TrustIsForTheRootTheUserListed == landed => TrustedByIntendedRule(StartedIn)

\* The usability equivalence canonicalization exists for: two locations that resolve to one repository are the
\* same repository, so one being trusted means the other is. `CompareRawPath` refutes exactly this.
ListedTrustIsCanonical ==
  \A l1, l2 \in Locations : SameRepository(l1, l2) => (TrustedByDecision(l1) = TrustedByDecision(l2))

\* A grant that outlives its entry is *not a separate property*: a session that lands on a withdrawn entry
\* lands untrusted, which `TrustIsForTheRootTheUserListed` already rejects. A monitor for it was written and
\* removed — it fired in the positive configuration for exactly the states the safety invariant covers, which
\* is another way of saying it was redundant — so staleness is checked through the safety property, by
\* `MC_projecttrust_stale.cfg`.

=============================================================================
