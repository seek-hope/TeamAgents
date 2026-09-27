------------------------------ MODULE V2Jobs -------------------------------
(***************************************************************************)
(* The job handshake and the recovery readVerdict (plan §6.2/§6.3, A10/A11, and  *)
(* the cancel races of A13; D-91/D-112/D-153). One controlled runner per      *)
(* active shell command journals its start handshake — READY ->               *)
(* START_ACCEPTED -> RUNNING -> terminal — so a crash never has to guess      *)
(* whether a side effect happened. The rules the code and the documents state *)
(* are:                                                                      *)
(*                                                                           *)
(*   * accept first, persist before any spawn: a command that ran is always   *)
(*     behind a journal that recorded the accepted start, so no run is        *)
(*     invisible to recovery (`NoEffectBeforeAccept`);                       *)
(*   * a duplicate GO never starts a second command (`AtMostOneExecutor`);    *)
(*   * CANCEL before the start persists CANCELLED_BEFORE_START and then       *)
(*     permanently rejects a late or replayed GO (`LateGoIsRejected`);        *)
(*   * a dead runner is judged from the persisted journal *and the effect     *)
(*     state at the moment of the read*: READY with no effect proves "did not  *)
(*     run", the START_ACCEPTED/RUNNING/CANCEL_REQUESTED band proves nothing   *)
(*     and is OUTCOME_UNKNOWN, and a terminal journal carries its own outcome  *)
(*     (`UnverifiableStartIsNeverGuessedNotRun`, `NotRunMeansNoEffect`) —     *)
(*     modelled as one atomic read, because a readVerdict that outlived its read   *)
(*     would be a readVerdict about a state nobody looked at;                     *)
(*   * a settled job's runner goes away instead of idling forever             *)
(*     (`SettledRunnerLeaves`, under weak fairness of the shutdown step —     *)
(*     D-153's leaked runners are exactly the jobs whose runner never left).  *)
(*                                                                           *)
(* Code anchors: engine/src/jobs/mod.rs — the phase list (`Journal::is_termi` *)
(* nal`) and the recovery contract in the module doc; engine/src/jobs/        *)
(* runner.rs — the `"go"` arm accepts only from READY and persists            *)
(* START_ACCEPTED *before* `start_command()` spawns, every other state is the *)
(* duplicate-GO no-op arm, the cancel arm persists CANCELLED_BEFORE_START     *)
(* first, and the recovery path rewrites the unverifiable band to             *)
(* OUTCOME_UNKNOWN with `starts`/`command_hash` beside it; engine/src/jobs/   *)
(* client.rs — `go` is idempotent and `persisted_journal` is what judges a    *)
(* dead runner. The live halves are review/dogfood/job_identity.py (A15/A10:  *)
(* pid/start_ticks/boot_id re-derived from /proc, a duplicate GO, a guessed   *)
(* job token) and the crash probes (crash.py, unknown_outcome.py, cancel.py). *)
(*                                                                           *)
(* What the model is and is not: the three identity fields (pid,             *)
(* start_ticks, boot_id) are abstracted into one `runner` variable — `alive`  *)
(* means "the recorded identity verifies against the process table", which is *)
(* what the code's comparison decides — and control requests are assumed      *)
(* authenticated, because the abstract socket name is derived from the job    *)
(* token (a transport property, not a journal one: a guessed name never      *)
(* reaches the runner). It does not model the deadline, the TERM->KILL        *)
(* escalation, the identity refusal (`job_id`/`command_hash` against the job  *)
(* file), the journal's bytes or the output files: those are the runner's own *)
(* tests and the live probes. `starts` is a real journal field, not a         *)
(* monitor: the model's `started` is that counter.                            *)
(*                                                                           *)
(* The four counterfactuals are the defects the rules exist against:          *)
(* `GuessNotRun` (a missing pid read as "did not run": the pre-D-91 shape),   *)
(* `DoubleGo` (a duplicate GO starting a second command), `LateGoStarts` (a   *)
(* late GO after CANCELLED_BEFORE_START starting it anyway) and               *)
(* `SpawnBeforeAccept` (a command started before its acceptance was           *)
(* persisted — the ordering `NoEffectBeforeAccept` is about).                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS GuessNotRun,       \* counterfactual: a dead runner is judged "did not run" without the journal
          DoubleGo,          \* counterfactual: a duplicate GO starts a second command
          LateGoStarts,      \* counterfactual: a GO after CANCELLED_BEFORE_START starts it anyway
          SpawnBeforeAccept  \* counterfactual: the command starts before its acceptance is persisted

Phases == {"READY", "START_ACCEPTED", "RUNNING", "CANCEL_REQUESTED",
           "SUCCEEDED", "FAILED", "CANCELLED", "CANCELLED_BEFORE_START", "OUTCOME_UNKNOWN"}
Terminal == {"SUCCEEDED", "FAILED", "CANCELLED", "CANCELLED_BEFORE_START", "OUTCOME_UNKNOWN"}
\* the band a dead runner cannot be judged from: a start was accepted, so nothing is provable
Unverifiable == {"START_ACCEPTED", "RUNNING", "CANCEL_REQUESTED"}
\* what the recovered child's exit code may become, CANCELLED included (the cancel race)
Outcomes == {"SUCCEEDED", "FAILED", "CANCELLED"}
Verdicts == {"none", "not-run", "unknown", "ran"}
ReadJournals == Phases \cup {"none"}   \* what a read can have been about

VARIABLES
  journal,        \* what the persisted journal says
  runner,         \* the runner process: alive (its recorded identity verifies) | dead
  started,        \* the journal's `starts` counter: how often a command was accepted and spawned
  effect,         \* did the command's side effect happen
  readJournal,    \* the journal a recovering client read (none = no read yet)
  readEffect,     \* the effect in that same snapshot: the classification is about the state it read
  readVerdict,    \* what that read classified the job as (none = no read yet)
  lateGoStarted   \* monitor: a late GO after CANCELLED_BEFORE_START started a command

vars == <<journal, runner, started, effect, readJournal, readEffect, readVerdict>>
monVars == <<vars, lateGoStarted>>

\* ------------------------------------------------------------------- actions --
\* The driver starts the runner; the runner persists READY before it answers a client.
StartRunner ==
  /\ runner = "dead"
  /\ journal = "READY"
  /\ runner' = "alive"
  /\ UNCHANGED <<journal, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* A first GO: accept from READY, persist START_ACCEPTED, and only then spawn (§6.2).
AcceptGo ==
  /\ runner = "alive"
  /\ journal = "READY"
  /\ journal' = "START_ACCEPTED"
  /\ started' = started + 1
  /\ UNCHANGED <<runner, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

SpawnCommand ==
  /\ runner = "alive"
  /\ journal = "START_ACCEPTED"
  /\ journal' = "RUNNING"
  /\ UNCHANGED <<runner, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* The command's side effect, only while the journal says it is running.
RunCommand ==
  /\ runner = "alive"
  /\ journal = "RUNNING"
  /\ effect = FALSE
  /\ effect' = TRUE
  /\ UNCHANGED <<journal, runner, started, readJournal, readEffect, readVerdict, lateGoStarted>>

\* The command ended: the runner journals a terminal state (CANCELLED when a
\* cancel was requested, so a real effect is never reported as a plain success).
Finish ==
  /\ runner = "alive"
  /\ journal \in {"RUNNING", "CANCEL_REQUESTED"}
  /\ \E outcome \in Outcomes : journal' = outcome
  /\ UNCHANGED <<runner, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* CANCEL that arrives before the command started: persist CANCELLED_BEFORE_START first.
CancelBeforeStart ==
  /\ runner = "alive"
  /\ journal = "READY"
  /\ journal' = "CANCELLED_BEFORE_START"
  /\ UNCHANGED <<runner, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* CANCEL while it runs: a stop request, then the terminal state.
RequestCancel ==
  /\ runner = "alive"
  /\ journal = "RUNNING"
  /\ journal' = "CANCEL_REQUESTED"
  /\ UNCHANGED <<runner, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* A duplicate GO is idempotent (`client::go`): the same journal comes back and no
\* second command starts. The counterfactual is the defect; the monitor saturates
\* (the invariant it breaks has already been broken).
DuplicateGo ==
  /\ runner = "alive"
  /\ journal \in Unverifiable \cup Terminal
  /\ started' = IF DoubleGo /\ started < 2 THEN started + 1 ELSE started
  /\ UNCHANGED <<journal, runner, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* A GO that arrives after CANCELLED_BEFORE_START is refused for good.
RejectLateGo ==
  /\ runner = "alive"
  /\ journal = "CANCELLED_BEFORE_START"
  /\ ~LateGoStarts
  /\ UNCHANGED monVars

\* The counterfactual: the cancel-before-start did not stay final.
StartLateGo ==
  /\ runner = "alive"
  /\ journal = "CANCELLED_BEFORE_START"
  /\ LateGoStarts
  /\ journal' = "START_ACCEPTED"
  /\ started' = IF started < 2 THEN started + 1 ELSE started
  /\ lateGoStarted' = TRUE
  /\ UNCHANGED <<runner, effect, readJournal, readEffect, readVerdict>>

\* The counterfactual the ordering forbids: the command ran, but no acceptance
\* was ever persisted, so the journal still says READY — invisible to recovery.
StartBeforeAccepting ==
  /\ runner = "alive"
  /\ journal = "READY"
  /\ SpawnBeforeAccept
  /\ effect' = TRUE
  /\ UNCHANGED <<journal, runner, started, readJournal, readEffect, readVerdict, lateGoStarted>>

\* The runner dies (crash or kill). The journal keeps what it last said.
CrashRunner ==
  /\ runner = "alive"
  /\ runner' = "dead"
  /\ UNCHANGED <<journal, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* D-153: once the job is settled a runner adds nothing, so it goes away.
StopRunner ==
  /\ runner = "alive"
  /\ journal \in Terminal
  /\ runner' = "dead"
  /\ UNCHANGED <<journal, started, effect, readJournal, readEffect, readVerdict, lateGoStarted>>

\* Recovery (§6.3, A11): read the dead runner's journal and classify. READY is the
\* only state that proves no start; the unverifiable band proves nothing and is
\* OUTCOME_UNKNOWN; a terminal journal carries its own outcome.
Recover ==
  /\ runner = "dead"
  /\ readJournal' = journal
  /\ readEffect' = effect
  /\ readVerdict' = IF journal = "READY" THEN "not-run"
                     ELSE IF journal \in Unverifiable /\ GuessNotRun THEN "not-run"
                     ELSE IF journal \in Unverifiable THEN "unknown"
                     ELSE "ran"
  /\ UNCHANGED <<journal, runner, started, effect, lateGoStarted>>

Stutter == UNCHANGED monVars

Next ==
  \/ StartRunner
  \/ AcceptGo
  \/ SpawnCommand
  \/ RunCommand
  \/ Finish
  \/ CancelBeforeStart
  \/ RequestCancel
  \/ DuplicateGo
  \/ RejectLateGo
  \/ StartLateGo
  \/ StartBeforeAccepting
  \/ CrashRunner
  \/ StopRunner
  \/ Recover
  \/ Stutter

Init ==
  /\ journal = "READY"
  /\ runner = "dead"
  /\ started = 0
  /\ effect = FALSE
  /\ readJournal = "none"
  /\ readEffect = FALSE
  /\ readVerdict = "none"
  /\ lateGoStarted = FALSE

\* Weak fairness asks the two ends of a job's life to happen: the runner starts,
\* and a settled job's runner leaves (D-153).
Spec == Init /\ [][Next]_monVars /\ WF_monVars(StartRunner) /\ WF_monVars(StopRunner)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ journal \in Phases
  /\ runner \in {"alive", "dead"}
  /\ started \in 0..2
  /\ effect \in BOOLEAN
  /\ readJournal \in ReadJournals
  /\ readEffect \in BOOLEAN
  /\ readVerdict \in Verdicts
  /\ lateGoStarted \in BOOLEAN

\* A10: at most one authorized executor. `started` is the journal's own `starts`
\* counter, so a second command — a duplicate GO, or a restart of a job whose
\* start cannot be verified — shows up here.
AtMostOneExecutor == started <= 1

\* A10/A13: cancel before the start is final; no late or replayed GO starts it.
LateGoIsRejected == lateGoStarted = FALSE

\* A10/A11: "did not run" is a proof, not a guess — only a journal with no
\* accepted start supports it, and the unverifiable band is OUTCOME_UNKNOWN. The
\* claim is about the read (the snapshot), not about the journal as it is later:
\* a job read as "did not run" may legitimately be re-armed and run afterwards.
UnverifiableStartIsNeverGuessedNotRun == readVerdict = "not-run" => readJournal = "READY"

\* The same claim from the effect's side: a "did not run" classification never
\* stands in front of a command that had already run when it was read.
NotRunMeansNoEffect == readVerdict = "not-run" => readEffect = FALSE

\* The ordering itself — the reason the accept is persisted before the spawn.
NoEffectBeforeAccept == effect = TRUE => journal # "READY"

\* A side effect always has an accepted start behind it.
EffectImpliesAcceptedStart == effect = TRUE => started >= 1

\* --------------------------------------------------------------- properties --
\* D-153: every settled job's runner leaves (the leaked runners the host cleanup
\* counts are the jobs whose runner never stopped).
SettledRunnerLeaves == [](journal \in Terminal => <>(runner = "dead"))

=============================================================================
