------------------------------ MODULE V2Prompt -------------------------------
(***************************************************************************)
(* What a member's prompt carries (D-102 recorded the promise, D-246         *)
(* delivered it): the session's `instruction_files` reach **every**          *)
(* instance's system text, whatever that instance's own profile says, and a  *)
(* file edited mid-session reaches the next turn.                           *)
(*                                                                          *)
(* The rules the code and the documents state:                              *)
(*                                                                          *)
(*   * an instance that runs a turn is prompted *with* the session's rules   *)
(*     (`EverybodyHasTheRules`) — a child spawned mid-session included,      *)
(*     which is the half a profile-row check cannot see;                     *)
(*   * a prompt is composed from the rules as they are *now*, not as they    *)
(*     were when the instance (or the session) started                       *)
(*     (`PromptsFollowTheCurrentRules`), so an edit lands on the next turn;  *)
(*   * a rules file that cannot be read is noted, never silently dropped     *)
(*     (`UnreadableRulesAreNoted` — `doctor` counts what it can read and the *)
(*     driver names the rest on its log).                                    *)
(*                                                                          *)
(* Code anchors: engine/src/config.rs `instruction_text` (in config order,   *)
(* each file under a heading naming it, unreadable paths returned to the     *)
(* caller) and `UserConfig::instruction_files`; engine/src/v2/driver.rs      *)
(* `team_kernel` — the one place an instance's system text is composed, so   *)
(* the leader and every child pass through it, and it composes per turn.     *)
(* The live half is `review/dogfood/instructions.py` (credential-free since  *)
(* D-246): a local server answers the leader with a `spawn` call and the     *)
(* probe reads the captured requests, so the assertion is on the bytes the   *)
(* provider received; `config::instruction_text_…` covers the composition.   *)
(*                                                                          *)
(* What the model is and is not: two instances (a leader and a member        *)
(* spawned mid-session), a small revision bound for the rules file, and the  *)
(* three counterfactuals below. It does not model the prompt's text beyond   *)
(* "the rules are in it", the sizes or the context window, or which layer    *)
(* (profile, grants, bindings) decided the rest of the text — those are      *)
(* `V2Grants` and the tools catalogue. Safety only: whether a user ever      *)
(* edits the file is not a liveness property, so there is no fairness        *)
(* assumption.                                                              *)
(*                                                                          *)
(* The three counterfactuals are the defects the rules exist against:        *)
(* `MembersUseTheirProfile` (a member's prompt is its own profile only — the *)
(* shape D-102 measured), `RulesFrozenAtStart` (the composition uses the     *)
(* rules as of the session's start, so an edit never lands) and              *)
(* `IgnoreUnreadable` (an unreadable file is dropped without a note).        *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Instances,             \* the session's instances, e.g. {"i-leader","i-worker"}
          Leader,                \* the instance whose profile carries the leader text
          RulesBound,            \* how many edits the model needs (>= 1 for non-vacuity)
          MaxTurns,              \* how many turns per instance the model needs (bounds the counter)
          RulesUnreadable,       \* the configured file cannot be read at all
          MembersUseTheirProfile,\* counterfactual: a member's prompt is its own profile only
          RulesFrozenAtStart,    \* counterfactual: the prompt uses the rules as of session start
          IgnoreUnreadable       \* counterfactual: an unreadable file is dropped without a note

ASSUME Instances # {} /\ Leader \in Instances /\ RulesBound >= 1 /\ MaxTurns >= 1

VARIABLES
  created,       \* instances that exist
  turns,         \* instance -> turns it has run (a prompt was composed each time)
  rulesRev,      \* the current rules revision (a user edit increments it)
  promptRev,     \* instance -> the revision its last prompt was composed from
  withoutRules,  \* monitor: instances that were prompted *without* the rules
  stalePrompt,   \* monitor: a prompt was composed from an outdated revision
  silent         \* monitor: an unreadable rules file was dropped without a note

vars == <<created, turns, rulesRev, promptRev, withoutRules, stalePrompt, silent>>

\* ------------------------------------------------------------------- actions --
\* A member appears: the leader's own instance, or a child spawned mid-session (possibly long after the session
\* start, which is what makes "the rules reach every member" a statement about *when* a prompt is composed).
Spawn(i) ==
  /\ i \notin created
  /\ created' = created \cup {i}
  /\ UNCHANGED <<turns, rulesRev, promptRev, withoutRules, stalePrompt, silent>>

\* The user edits the rules file: the revision moves, and nothing already prompted is retracted — an instance
\* that has not run since simply has not been prompted again yet.
EditRules ==
  /\ rulesRev < RulesBound
  /\ rulesRev' = rulesRev + 1
  /\ UNCHANGED <<created, turns, promptRev, withoutRules, stalePrompt, silent>>

\* One turn of one instance: its system text is composed. `team_kernel` composes the rules in for *every*
\* instance (that is the shipped behaviour); the counterfactuals are the two ways to get it wrong, and the
\* unreadable case is the third.
Compose(i) ==
  /\ i \in created
  /\ turns[i] < MaxTurns
  \* which of the two shapes this prompt takes: the shipped one composes the session's rules in for every
  \* instance; the counterfactual gives a member its own profile only (D-102's measurement)
  /\ \/ /\ ~(MembersUseTheirProfile /\ i # Leader)
        /\ withoutRules' = withoutRules
     \/ /\ MembersUseTheirProfile /\ i # Leader
        /\ withoutRules' = withoutRules \cup {i}
  /\ turns' = [turns EXCEPT ![i] = turns[i] + 1]
  /\ promptRev' = [promptRev EXCEPT ![i] = IF RulesFrozenAtStart THEN 0 ELSE rulesRev]
  \* the parentheses matter: `=` binds tighter than `\\/`, so an unparenthesised right-hand disjunction would
  \* satisfy the conjunct *without* assigning the variable — TLC answered "Successor state is not completely
  \* specified … not assigned: stalePrompt", which is how the first draft of this action was caught (the base
  \* configuration verified, because both constants are FALSE there)
  /\ stalePrompt' = (stalePrompt \/ (RulesFrozenAtStart /\ rulesRev > 0))
  /\ silent' = (silent \/ (RulesUnreadable /\ IgnoreUnreadable))
  /\ UNCHANGED <<created, rulesRev>>

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED vars

Next ==
  \/ \E i \in Instances : Spawn(i)
  \/ EditRules
  \/ \E i \in Instances : Compose(i)
  \/ Stutter

Init ==
  /\ created = {}
  /\ turns = [i \in Instances |-> 0]
  /\ rulesRev = 0
  /\ promptRev = [i \in Instances |-> 0]
  /\ withoutRules = {}
  /\ stalePrompt = FALSE
  /\ silent = FALSE

Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ created \subseteq Instances
  /\ turns \in [Instances -> 0..MaxTurns]
  /\ rulesRev \in 0..RulesBound
  /\ promptRev \in [Instances -> 0..RulesBound]
  /\ withoutRules \subseteq Instances
  /\ stalePrompt \in BOOLEAN
  /\ silent \in BOOLEAN

\* D-246: an instance that has run a turn was prompted with the session's rules — the leader and every child,
\* however late the child appeared (`MC_prompt_members_use_their_profile.cfg` is the D-102 shape that breaks it).
EverybodyHasTheRules == withoutRules = {}

\* An edit reaches the next turn: no prompt is composed from an older revision
\* (`MC_prompt_rules_frozen_at_start.cfg` breaks this one).
PromptsFollowTheCurrentRules == stalePrompt = FALSE

\* A rules file that cannot be read is named (`doctor` counts what it can read, the driver the log) — never
\* dropped in silence (`MC_prompt_ignore_unreadable.cfg` breaks this one).
UnreadableRulesAreNoted == silent = FALSE

=============================================================================
