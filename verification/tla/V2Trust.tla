------------------------------ MODULE V2Trust -------------------------------
(***************************************************************************)
(* The repository-local config and the one gate that admits it (D-244).        *)
(*                                                                           *)
(* A session reads two files: the user's own config and, for the directory it  *)
(* works in, `<cwd>/.teamagents/config.toml`. What a *cloned repository* may   *)
(* put into a session is the question this module answers, and the rules the   *)
(* code and the documents state are:                                          *)
(*                                                                           *)
(*   * nothing a project defines reaches the session unless the *user* asked   *)
(*     for it (`NothingFromTheProjectUntrusted`): a project-sourced model      *)
(*     profile carries `base_url`/`api_key_env`, so without that gate a repo   *)
(*     could become the default model (the "exactly one configured key"        *)
(*     fallback) and be handed the user's credential;                          *)
(*   * the user's own definition of a name always wins                         *)
(*     (`UserDefinitionsNeverOverridden`);                                    *)
(*   * `[permissions]` (the mode and the trust flag itself), hooks, checks,    *)
(*     retention and limits are the user's, trusted or not                     *)
(*     (`PolicyClassesStayTheUsers`) — a check is a command that runs without  *)
(*     an approval prompt and a limit is the ceiling the user set;             *)
(*   * the trust decision has exactly one source: the user's own config        *)
(*     (`TrustOnlyFromTheUser`) — a project's `[permissions]` table is refused *)
(*     by name, so a repository cannot grant itself the gate;                  *)
(*   * a refused entry is *named*, never dropped in silence                    *)
(*     (`RefusalsAreNamed`, what `ProjectMerge.refused` is for).               *)
(*                                                                           *)
(* Code anchors: engine/src/config.rs — `project_permissions` (the one reader  *)
(* of `[permissions]`, always handed the *user* value), `load_user_config_for` *)
(* (the merge: models/tools/skills_paths/instruction_files behind `trusted`,   *)
(* the `retention`/`hooks`/`checks`/`limits` loop that copies only the user's, *)
(* and every refusal pushed into `ProjectMerge.refused`), and                *)
(* `read_config_file` (an unreadable file is an error, not an empty one).      *)
(* The live half is `review/dogfood/project_config.py`: the offered-surface    *)
(* witness twice, refused and trusted, plus a project that tries to grant      *)
(* itself the flag. The tests are `config::project_config_tests` (the two      *)
(* loaders agree; an untrusted project lands nothing and says so; a project    *)
(* cannot set the mode or grant itself trust).                                *)
(*                                                                           *)
(* What the model is and is not: one merge over a small pool of named entries  *)
(* (`Names` for models and tools, `Paths` for skills and instruction files,    *)
(* `PolicyClasses` for the user-only classes) and the five counterfactuals     *)
(* below. It does not model where the paths point, what a tool binding *does*, *)
(* or the file syntax (an unknown key or a broken table is a load error in the *)
(* code, which the tests cover); "the merge" is a step here, not the reading   *)
(* of two files. Safety only: whether a user ever trusts a repository is not a *)
(* liveness property, so there is no fairness assumption.                      *)
(*                                                                           *)
(* The five counterfactuals are the defects the rules exist against:           *)
(* `ProjectCanGrantTrust` (the project's own table sets the flag),             *)
(* `ModelsIgnoreTrust` (model profiles merge with no gate, the rule D-244      *)
(* narrowed), `ProjectWinsOnNameClash` (a project entry replaces the user's),  *)
(* `ProjectMaySetPolicy` (hooks/checks/limits from the project too) and        *)
(* `RefuseSilently` (a refused entry dropped without a note).                  *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Names,                 \* catalog names a config can define (models and tools)
          Paths,                 \* configured skills/instruction paths
          PolicyClasses,         \* the classes only the user may set
          ProjectCanGrantTrust,  \* counterfactual: the project's [permissions] sets the flag
          ModelsIgnoreTrust,     \* counterfactual: model profiles merge with no gate
          ProjectWinsOnNameClash,\* counterfactual: a project entry replaces the user's
          ProjectMaySetPolicy,   \* counterfactual: policy classes come from the project too
          RefuseSilently         \* counterfactual: a refused entry is dropped with no note

ASSUME Names # {} /\ Paths # {} /\ PolicyClasses # {}

Sources == {"none", "user", "project"}

VARIABLES
  models,        \* the effective model catalog: Names -> Sources
  tools,         \* the effective tool bindings: Names -> Sources
  skills,        \* the effective skills_paths: Paths -> Sources
  instructions,  \* the effective instruction_files: Paths -> Sources
  policy,        \* the user-only classes: PolicyClasses -> Sources
  userModels,    \* the names the user defined (never taken back): a set
  trustAsked,    \* the user asked for trust — the only source of `trusted`
  trusted,       \* the resolved gate
  refused,       \* a refusal happened at all (the merge had an entry it did not take)
  silent         \* ...and it was dropped without being recorded — the counterfactual

vars == <<models, tools, skills, instructions, policy, userModels, trustAsked, trusted, refused, silent>>

\* ------------------------------------------------------------------- actions --
\* The user's own config: it defines names, sets the user-only classes, and it is the one file whose
\* `[permissions]` table answers the trust question.
UserDefinesModel(n) ==
  /\ models[n] = "none"
  /\ models' = [models EXCEPT ![n] = "user"]
  /\ userModels' = userModels \cup {n}
  /\ UNCHANGED <<tools, skills, instructions, policy, trustAsked, trusted, refused, silent>>

UserDefinesTool(n) ==
  /\ tools[n] = "none"
  /\ tools' = [tools EXCEPT ![n] = "user"]
  /\ UNCHANGED <<models, skills, instructions, policy, userModels, trustAsked, trusted, refused, silent>>

UserDefinesPath(p) ==
  /\ skills[p] = "none"
  /\ instructions[p] = "none"
  /\ skills' = [skills EXCEPT ![p] = "user"]
  /\ instructions' = [instructions EXCEPT ![p] = "user"]
  /\ UNCHANGED <<models, tools, policy, userModels, trustAsked, trusted, refused, silent>>

UserDefinesPolicy(c) ==
  /\ policy[c] = "none"
  /\ policy' = [policy EXCEPT ![c] = "user"]
  /\ UNCHANGED <<models, tools, skills, instructions, userModels, trustAsked, trusted, refused, silent>>

\* `[permissions] trust_project = true` in the user's own config — the one action that may set the gate.
UserOffersTrust ==
  /\ ~trustAsked
  /\ trustAsked' = TRUE
  /\ trusted' = TRUE
  /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels, refused, silent>>

\* The refusal branch of an action: the entry is recorded (the base rule) or dropped in silence — the shape
\* `ProjectMerge.refused` and `doctor`'s row exist against. Which entry was refused is prose the code prints;
\* the model keeps only "a refusal happened" and whether it was recorded.
Refusal ==
  /\ refused' = TRUE
  /\ silent' = (silent \/ RefuseSilently)
  /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels, trustAsked, trusted>>

\* The project file defines a model profile. The gate is `trusted`, unless the counterfactual removes it
\* (the rule D-244 narrowed); a name the user defined is refused, or — counterfactually — replaced.
ProjectDefinesModel(n) ==
  \/ /\ (ModelsIgnoreTrust \/ trusted)
     /\ models[n] = "none"
     /\ models' = [models EXCEPT ![n] = "project"]
     /\ UNCHANGED <<tools, skills, instructions, policy, userModels, trustAsked, trusted, refused, silent>>
  \/ /\ ProjectWinsOnNameClash
     /\ models[n] = "user"
     /\ models' = [models EXCEPT ![n] = "project"]
     /\ UNCHANGED <<tools, skills, instructions, policy, userModels, trustAsked, trusted, refused, silent>>
  \/ /\ models[n] # "none"
     /\ Refusal
     /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels, trustAsked, trusted>>

ProjectDefinesTool(n) ==
  \/ /\ trusted
     /\ tools[n] = "none"
     /\ tools' = [tools EXCEPT ![n] = "project"]
     /\ UNCHANGED <<models, skills, instructions, policy, userModels, trustAsked, trusted, refused, silent>>
  \/ /\ tools[n] # "none"
     /\ Refusal
     /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels, trustAsked, trusted>>

ProjectDefinesPath(p) ==
  \/ /\ trusted
     /\ skills[p] = "none"
     /\ skills' = [skills EXCEPT ![p] = "project"]
     /\ instructions' = instructions
     /\ UNCHANGED <<models, tools, policy, userModels, trustAsked, trusted, refused, silent>>
  \/ /\ skills[p] # "none"
     /\ Refusal
     /\ UNCHANGED <<models, tools, instructions, policy, userModels, trustAsked, trusted>>

\* The project's `[permissions]` table: refused by name always, and it can never set the gate.
ProjectSetsPermissions ==
  \/ /\ ~ProjectCanGrantTrust
     /\ trusted' = trusted
     /\ Refusal
     /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels, trustAsked>>
  \/ /\ ProjectCanGrantTrust
     /\ trusted' = TRUE
     /\ trustAsked' = trustAsked
     /\ refused' = refused
     /\ silent' = silent
     /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels>>

\* A user-only class (hooks, checks, retention, limits, `[permissions] mode`) from the project file.
ProjectSetsPolicy(c) ==
  \/ /\ ~ProjectMaySetPolicy /\ policy[c] = "none"
     /\ Refusal
     /\ UNCHANGED <<models, tools, skills, instructions, policy, userModels, trustAsked, trusted>>
  \/ /\ ProjectMaySetPolicy
     /\ policy[c] = "none"
     /\ policy' = [policy EXCEPT ![c] = "project"]
     /\ UNCHANGED <<models, tools, skills, instructions, userModels, trustAsked, trusted, refused, silent>>

Stutter == UNCHANGED vars

Next ==
  \/ \E n \in Names : UserDefinesModel(n)
  \/ \E n \in Names : UserDefinesTool(n)
  \/ \E p \in Paths : UserDefinesPath(p)
  \/ \E c \in PolicyClasses : UserDefinesPolicy(c)
  \/ UserOffersTrust
  \/ \E n \in Names : ProjectDefinesModel(n)
  \/ \E n \in Names : ProjectDefinesTool(n)
  \/ \E p \in Paths : ProjectDefinesPath(p)
  \/ ProjectSetsPermissions
  \/ \E c \in PolicyClasses : ProjectSetsPolicy(c)
  \/ Stutter

Init ==
  /\ models = [n \in Names |-> "none"]
  /\ tools = [n \in Names |-> "none"]
  /\ skills = [p \in Paths |-> "none"]
  /\ instructions = [p \in Paths |-> "none"]
  /\ policy = [c \in PolicyClasses |-> "none"]
  /\ userModels = {}
  /\ trustAsked = FALSE
  /\ trusted = FALSE
  /\ refused = FALSE
  /\ silent = FALSE

Spec == Init /\ [][Next]_vars

ProjectLanded ==
  \/ \E n \in Names : models[n] = "project"
  \/ \E n \in Names : tools[n] = "project"
  \/ \E p \in Paths : skills[p] = "project" \/ instructions[p] = "project"

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ models \in [Names -> Sources]
  /\ tools \in [Names -> Sources]
  /\ skills \in [Paths -> Sources]
  /\ instructions \in [Paths -> Sources]
  /\ policy \in [PolicyClasses -> Sources]
  /\ userModels \subseteq Names
  /\ trustAsked \in BOOLEAN
  /\ trusted \in BOOLEAN
  /\ refused \in BOOLEAN
  /\ silent \in BOOLEAN
\* D-244's rule: without the user's opt-in the repository's file lands nothing at all.
NothingFromTheProjectUntrusted == trusted \/ ~ProjectLanded

\* The user's own definition of a name is never replaced by a project entry.
UserDefinitionsNeverOverridden == \A n \in userModels : models[n] = "user"

\* The user-only classes never take a project value, trusted or not.
PolicyClassesStayTheUsers == \A c \in PolicyClasses : policy[c] # "project"

\* The gate has one source: the user's own `[permissions]` table.
TrustOnlyFromTheUser == trusted => trustAsked

\* Every refusal is named (`ProjectMerge.refused`), never dropped in silence.
RefusalsAreNamed == ~(refused /\ silent)

=============================================================================
