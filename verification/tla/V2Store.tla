------------------------------ MODULE V2Store -------------------------------
(***************************************************************************)
(* Session-store identity (plan §4.4, A34; D-87): opening a state root either  *)
(* owns the file or refuses it. It never stamps a database that belongs to     *)
(* someone else, a refusal writes nothing at all, and a database interrupted   *)
(* between its schema batch and its stamp is completed rather than stranded.   *)
(*                                                                          *)
(* Code anchors: core/src/v2/store.rs — open(path, create) reads the stamp     *)
(* through `sqlite_master` and never creates it, applies journal_mode and      *)
(* synchronous only after the format check accepted the file, refuses a file   *)
(* whose stamp is not `teamagents-v2`, refuses an unstamped file that holds    *)
(* tables the SCHEMA does not create (`foreign_tables` against               *)
(* `schema_tables`, which is derived from the schema text), and otherwise     *)
(* writes the schema and stamps the file. A crash between those two writes     *)
(* leaves exactly `tables = OurTables /\ stamp = "none"`, the state the next   *)
(* open completes.                                                            *)
(*                                                                          *)
(* What the model is and is not: `lastWrote` is the model's claim that the     *)
(* step wrote to the file, and `RefusalsWriteNothing` states that a refusal    *)
(* never does. The stronger, byte-level version of that claim lives in the     *)
(* code's own test (`open_never_adopts_an_unstamped_file_that_holds_foreign_   *)
(* tables` compares the file with what it held before) and in               *)
(* `review/dogfood/boundary.py`, which hashes the file across a real `exec`.   *)
(*                                                                          *)
(* The counterfactual `IgnoreForeign` is the D-87 defect: `create = true`      *)
(* initializes whenever there is no stamp, foreign tables or not.             *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS OurTables,      \* the tables SCHEMA creates, e.g. {"meta","goals","events"}
          ForeignTables,  \* tables of some other program, e.g. {"users"}
          IgnoreForeign   \* counterfactual: skip the foreign-table guard (pre-D-87)

ASSUME OurTables # {} /\ ForeignTables # {} /\ OurTables \cap ForeignTables = {}

Stamps == {"none", "v2", "other"}
Outcomes == {"none", "accepted", "initialized", "interrupted",
             "refused-unstamped", "refused-foreign", "refused-format"}
Refusals == {"refused-unstamped", "refused-foreign", "refused-format"}

VARIABLES
  tables,          \* the tables the file holds
  stamp,           \* the format stamp: none | v2 | other
  lastOpen,        \* the outcome of the most recent open
  lastWrote,       \* did that open write to the file
  adoptedForeign   \* monitor: an open stamped a file that held foreign tables

vars == <<tables, stamp, lastOpen, lastWrote>>
monVars == <<vars, adoptedForeign>>

\* ------------------------------------------------------------------- actions --
\* Another program creates its own database in this path before any session
\* opens it. Only possible while the file holds nothing of ours.
ForeignProgram(m) ==
  /\ m \in ForeignTables
  /\ tables = {}
  /\ stamp = "none"
  /\ tables' = {m}
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<stamp, lastOpen, lastWrote>>

\* The same, with a format id of its own: the `refused-format` path.
ForeignProgramStamped(m) ==
  /\ m \in ForeignTables
  /\ tables = {}
  /\ stamp = "none"
  /\ tables' = {m}
  /\ stamp' = "other"
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<lastOpen, lastWrote>>

\* open(create = false) — doctor and the read-only checks: verify, never write.
OpenRead ==
  /\ lastOpen' = IF stamp = "v2" THEN "accepted"
                   ELSE IF stamp = "other" THEN "refused-format"
                   ELSE "refused-unstamped"
  /\ lastWrote' = FALSE
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<tables, stamp>>

\* open(create = true) on a stamped v2 file: verify the schema, no identity
\* decision (CREATE TABLE IF NOT EXISTS writes nothing new).
AcceptStamped ==
  /\ stamp = "v2"
  /\ lastOpen' = "accepted"
  /\ lastWrote' = TRUE
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<tables, stamp>>

\* A stamp that is not v2 is never reinterpreted: migrate-or-refuse (§4.4).
RefuseForeignFormat ==
  /\ stamp = "other"
  /\ lastOpen' = "refused-format"
  /\ lastWrote' = FALSE
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<tables, stamp>>

\* Unstamped and foreign: someone else's file, refused untouched (D-87).
RefuseForeignTables ==
  /\ stamp = "none"
  /\ tables \cap ForeignTables # {}
  /\ ~IgnoreForeign
  /\ lastOpen' = "refused-foreign"
  /\ lastWrote' = FALSE
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<tables, stamp>>

\* Unstamped and ours to take: write the schema and stamp it in one step.
InitializeOurs ==
  /\ stamp = "none"
  /\ (IgnoreForeign \/ tables \cap ForeignTables = {})
  /\ tables' = tables \cup OurTables
  /\ stamp' = "v2"
  /\ lastOpen' = "initialized"
  /\ lastWrote' = TRUE
  \* the monitor: with the guard off this is the defect itself
  /\ adoptedForeign' = (tables \cap ForeignTables # {})

\* The crash between the schema batch and the stamp insert: the file now holds
\* our tables without a stamp — the state the next open has to complete.
WriteSchemaThenCrash ==
  /\ stamp = "none"
  /\ (IgnoreForeign \/ tables \cap ForeignTables = {})
  /\ tables' = tables \cup OurTables
  /\ lastOpen' = "interrupted"
  /\ lastWrote' = TRUE
  /\ adoptedForeign' = adoptedForeign
  /\ UNCHANGED <<stamp>>

Stutter == UNCHANGED monVars

Next ==
  \/ \E m \in ForeignTables : ForeignProgram(m)
  \/ \E m \in ForeignTables : ForeignProgramStamped(m)
  \/ OpenRead
  \/ AcceptStamped
  \/ RefuseForeignFormat
  \/ RefuseForeignTables
  \/ InitializeOurs
  \/ WriteSchemaThenCrash
  \/ Stutter

Init ==
  /\ tables = {}
  /\ stamp = "none"
  /\ lastOpen = "none"
  /\ lastWrote = FALSE
  /\ adoptedForeign = FALSE

Spec == Init /\ [][Next]_monVars /\ WF_monVars(InitializeOurs)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ tables \subseteq OurTables \cup ForeignTables
  /\ stamp \in Stamps
  /\ lastOpen \in Outcomes
  /\ lastWrote \in BOOLEAN
  /\ adoptedForeign \in BOOLEAN

\* A34/D-87: a file that holds foreign tables is never stamped as ours, however
\* often it is opened. `adoptedForeign` monitors the step that would do it.
NoForeignAdoption == adoptedForeign = FALSE

\* The same claim as a state invariant over the file itself.
NoForeignStamp == stamp = "v2" => tables \cap ForeignTables = {}

\* A refusal is silent: it never writes to the file it refused.
RefusalIsSilent == lastOpen \in Refusals => lastWrote = FALSE

RefusalsWriteNothing == RefusalIsSilent

\* The crash path only ever leaves our own tables behind.
InterruptedWroteOursOnly == lastOpen = "interrupted" => tables \cap ForeignTables = {}

\* An accepted or initialized open ends on a v2 file.
AcceptedMeansStamped == lastOpen \in {"accepted", "initialized"} => stamp = "v2"

\* --------------------------------------------------------------- properties --
\* The invariant in temporal form: from every step on, no v2 stamp sits on a
\* file that holds foreign tables.
NoForeignStampAlways == []NoForeignStamp

\* Every refusal (in every step) is silent.
EveryRefusalIsSilent == [][RefusalIsSilent]_vars

\* A database interrupted between its schema and its stamp is not stranded: with
\* the initializer weakly fair, the next open completes it. (Reachable from
\* `WriteSchemaThenCrash` only, because the environment writes foreign tables
\* only into an empty file.)
HalfInitializedIsCompleted ==
  (stamp = "none" /\ tables \cap ForeignTables = {} /\ tables # {}) ~> (stamp = "v2")

=============================================================================
