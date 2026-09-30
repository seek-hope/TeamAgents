------------------------------ MODULE V2Schedule ----------------------------
(***************************************************************************)
(* Automations (D-367): user-defined schedules that start a goal on their   *)
(* own.                                                                     *)
(*                                                                          *)
(* An automation is enabled or paused, has a next due slot, and running     *)
(* means one of its runs is in flight. The rules the daemon guarantees are  *)
(* exactly what a scheduler can get wrong:                                *)
(*                                                                          *)
(*   - a slot is never run twice (`NoDuplicateRun`): beginning a run        *)
(*     advances `nextAt` in the same step;                                      *)
(*   - two runs of one automation never overlap                            *)
(*     (`AtMostOneRunPerAutomation`): a run starts only while none is in    *)
(*     flight;                                                              *)
(*   - a disabled automation never runs (`DisabledNeverRuns`); and          *)
(*   - a run starts only at a due slot (`OnlyDueSlotsRun`).                 *)
(*                                                                          *)
(* The model is safety only — a run need not ever happen — and it           *)
(* abstracts the period into "the slot advances by one"; the coalescing of  *)
(* missed slots and the goal the run opens are the code's, pinned by        *)
(* `automations.rs`'s tests and `cli::a_due_automation_opens_one_goal_and_   *)
(* does_not_overlap_itself`. The four negative controls at the end each     *)
(* forget one rule and must be refuted.                                     *)
(*                                                                          *)
(* Code anchors: engine/src/v2/automations.rs (`due`, `mark_run`,           *)
(* `set_enabled`) and engine/src/v2/daemon.rs `run_scheduler`.              *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS Automations,     \* automation ids, e.g. {"a1","a2"}
          MaxTime,         \* the time bound TLC needs
          AllowDuplicate,  \* negative control: a run does not advance the slot
          AllowOverlap,    \* negative control: a run starts while one is in flight
          AllowDisabled,   \* negative control: a paused automation runs
          AllowEarly       \* negative control: a run starts before its slot

ASSUME Automations # {} /\ MaxTime > 0

VARIABLES
  now,          \* the clock, bounded by MaxTime
  enabled,      \* id -> whether the automation is armed
  nextAt,       \* id -> the next due slot
  running,      \* id -> whether a run is in flight
  log,          \* the runs that happened, as [id, slot] records
  overlapped,   \* monitor: a run started while one was already in flight
  ranDisabled,  \* monitor: a paused automation ran
  early         \* monitor: a run started before its slot

vars == <<now, enabled, nextAt, running, log, overlapped, ranDisabled, early>>

\* ------------------------------------------------------------------ actions --
Tick ==
  /\ now < MaxTime
  /\ now' = now + 1
  /\ UNCHANGED <<enabled, nextAt, running, log, overlapped, ranDisabled, early>>

\* Begin a run. Every guard is a rule; the three monitors record the step where a control forgot one, so the
\* matching invariant is stated over the state rather than over a temporal formula.
Start(i) ==
  /\ i \in Automations
  /\ (AllowEarly \/ nextAt[i] <= now)
  /\ (AllowDisabled \/ enabled[i])
  /\ (AllowOverlap \/ ~running[i])
  /\ log' = Append(log, [id |-> i, slot |-> nextAt[i]])
  /\ overlapped' = (overlapped \/ running[i])
  /\ ranDisabled' = (ranDisabled \/ ~enabled[i])
  /\ early' = (early \/ (nextAt[i] > now))
  /\ running' = [running EXCEPT ![i] = TRUE]
  /\ nextAt' = IF AllowDuplicate THEN nextAt ELSE [nextAt EXCEPT ![i] = @ + 1]
  /\ UNCHANGED <<now, enabled>>

\* The run ends; the automation may run again at its next slot.
Finish(i) ==
  /\ running[i]
  /\ running' = [running EXCEPT ![i] = FALSE]
  /\ UNCHANGED <<now, enabled, nextAt, log, overlapped, ranDisabled, early>>

\* Pause an automation; its slot is kept.
Pause(i) ==
  /\ enabled[i]
  /\ enabled' = [enabled EXCEPT ![i] = FALSE]
  /\ UNCHANGED <<now, nextAt, running, log, overlapped, ranDisabled, early>>

\* Re-arm it: one slot out, so resuming never fires a stale schedule immediately.
Arm(i) ==
  /\ ~enabled[i]
  /\ enabled' = [enabled EXCEPT ![i] = TRUE]
  /\ nextAt' = [nextAt EXCEPT ![i] = now + 1]
  /\ UNCHANGED <<now, running, log, overlapped, ranDisabled, early>>

Stutter == UNCHANGED vars

Next ==
  \/ Tick
  \/ \E i \in Automations : Start(i)
  \/ \E i \in Automations : Finish(i)
  \/ \E i \in Automations : Pause(i)
  \/ \E i \in Automations : Arm(i)
  \/ Stutter

Init ==
  /\ now = 0
  /\ enabled = [ i \in Automations |-> TRUE ]
  /\ nextAt = [ i \in Automations |-> 0 ]
  /\ running = [ i \in Automations |-> FALSE ]
  /\ log = <<>>
  /\ overlapped = FALSE
  /\ ranDisabled = FALSE
  /\ early = FALSE

Spec == Init /\ [][Next]_vars

\* -------------------------------------------------------------- invariants --
TypeOK ==
  /\ now \in 0..MaxTime
  /\ \A i \in Automations : enabled[i] \in BOOLEAN
  /\ \A i \in Automations : nextAt[i] \in Nat
  /\ \A i \in Automations : running[i] \in BOOLEAN
  /\ log \in Seq([id : Automations, slot : Nat])
  /\ overlapped \in BOOLEAN
  /\ ranDisabled \in BOOLEAN
  /\ early \in BOOLEAN

\* A slot is never run twice: `nextAt` advances in the step that records the run.
NoDuplicateRun ==
  \A k, l \in 1..Len(log) : log[k] = log[l] => k = l

\* Two runs of one automation never overlap.
AtMostOneRunPerAutomation ==
  overlapped = FALSE

\* A disabled automation never runs.
DisabledNeverRuns ==
  ranDisabled = FALSE

\* A run starts only at a due slot.
OnlyDueSlotsRun ==
  early = FALSE

=============================================================================
