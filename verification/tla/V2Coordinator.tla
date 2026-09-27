------------------------------ MODULE V2Coordinator -------------------------------
(* The coordinator lock: one per state root, and the fork window (§6.1, A33).  *)
(*                                                                            *)
(* A33's second half is a rule about file descriptors, not about messages:    *)
(* "one coordinator only, and tools never inherit the coordinator lock in a   *)
(* way that blocks recovery". The code states both halves in one place —      *)
(* `state_lock` takes an OS file lock on `<state root>/coordinator.lock`, and *)
(* the comment above `LOCK_WAIT` explains what a `Command::spawn` does to it: *)
(* a tool child forks with the whole descriptor table inherited, so the       *)
(* child's copy keeps the lock alive until `exec` closes it (std sets         *)
(* CLOEXEC); a restart that lands in that instant must *wait* for it, because *)
(* the kernel still grants the lock to exactly one holder. The model checks:  *)
(*                                                                            *)
(*   * `AtMostOneCoordinator`: the lock is granted to one holder — a second   *)
(*     coordinator cannot take it while a live one holds it                   *)
(*     (`TwoCoordinators`, the monitor, stays false);                        *)
(*   * `TheWindowIsNotMistakenForAHolder`: a restart that finds only a        *)
(*     child's inherited copy waits instead of reporting a coordinator that   *)
(*     is not really there (`WronglyRefused` stays false) — the defect that   *)
(*     `a_momentarily_held_lock_is_absorbed` exists for;                      *)
(*   * `NoHeldLockWithoutAHolder`: once the fork window closes, nothing in    *)
(*     the product holds the lock, so a restart is never blocked by a tool    *)
(*     child (`StuckLock` stays false) — the other half of the rule, the      *)
(*     reason the descriptor is CLOEXEC at all;                               *)
(*   * and `RecoverySucceedsOnceNothingHoldsIt`: with the restart retried at  *)
(*     the code's poll pace, a free lock is eventually taken                 *)
(*     (`RecoveryIsLive`, under weak fairness of the attempt).                *)
(*                                                                            *)
(* Code anchors: engine/src/jobs/mod.rs — `LOCK_WAIT`/`LOCK_STEP` and their    *)
(* comment (the fork window, CLOEXEC at exec, "the kernel still grants the     *)
(* lock to exactly one holder, so exclusivity is unchanged"), `state_lock` /   *)
(* `state_lock_waiting` (the `WouldBlock` branch that sleeps and the bounded   *)
(* "state root already has a coordinator" refusal for a real holder), and the  *)
(* two tests `a_momentarily_held_lock_is_absorbed` and the second-coordinator  *)
(* one; engine/src/v2/driver.rs takes it at `state_lock(&config.state_root.    *)
(* join("coordinator.lock"))`, and `review/dogfood/boundary.py` is the live    *)
(* half (a second daemon, a restart after SIGKILL).                           *)
(*                                                                            *)
(* What the model is and is not: it abstracts the descriptor table to one     *)
(* counter — how many processes reference the description that holds the lock *)
(* — and one child (a shell-job runner, an MCP server or a hook: every        *)
(* `Command::spawn` behaves the same here). It does not model the poll        *)
(* interval, the file's path, the OS's own lock semantics beyond "exactly one *)
(* holder", or what the child does after exec; nor does it model the daemon's *)
(* shutdown path (`daemon --stop` is not a product lever yet: the user sends  *)
(* a signal, D-150).                                                          *)
(*                                                                            *)
(* The three counterfactuals are the defects the rules exist against:          *)
(* `ReportInsteadOfWaiting` (a restart reports a coordinator while only a      *)
(* child's copy holds the lock), `ChildKeepsLock` (the descriptor survives     *)
(* exec, so a long tool child blocks recovery) and `NotExclusive` (the kernel  *)
(* grants two holders).                                                       *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS ReportInsteadOfWaiting,  \* counterfactual: a restart refuses in the fork window instead of waiting
          ChildKeepsLock,          \* counterfactual: the child's descriptor survives exec
          NotExclusive             \* counterfactual: two coordinators may hold the lock

ChildPhases == {"none", "forked", "execd"}
Outcomes == {"none", "acquired", "waited", "refused-window", "refused-real"}

VARIABLES
  daemon,        \* the coordinator: "live" | "gone"
  child,         \* the tool child: none | forked (between fork and exec) | execd (past it)
  references,    \* processes referencing the description that holds the lock: 0, 1 or 2
  outcome,       \* the last classification a restart produced
  coordinators,  \* monitor: how many coordinators hold the lock
  wronglyRefused \* monitor: a restart refused although only a child's copy held the lock

vars == <<daemon, child, references, outcome>>
monVars == <<vars, coordinators, wronglyRefused>>

\* ------------------------------------------------------------------- actions --
\* The first coordinator takes a free lock (one reference: its own descriptor).
AcquireFree ==
  /\ references = 0
  /\ daemon = "gone"
  /\ daemon' = "live"
  /\ references' = 1
  /\ outcome' = "acquired"
  /\ coordinators' = coordinators + 1
  /\ UNCHANGED <<child, wronglyRefused>>

\* A second coordinator while a live one holds it: the kernel refuses, and the
\* code's bounded wait ends in the honest refusal.
AcquireHeldByDaemon ==
  /\ references > 0
  /\ daemon = "live"
  /\ ~NotExclusive
  /\ outcome' = "refused-real"
  /\ UNCHANGED <<daemon, child, references, coordinators, wronglyRefused>>

\* The counterfactual: exclusivity is not the kernel's.
AcquireAnyway ==
  /\ NotExclusive
  /\ daemon = "live"
  /\ daemon' = "live"
  /\ references' = references + 1
  /\ outcome' = "acquired"
  /\ coordinators' = coordinators + 1
  /\ UNCHANGED <<child, wronglyRefused>>

\* `Command::spawn`: the child forks with the descriptor table inherited, so for
\* that instant two processes reference the description that holds the lock.
ForkChild ==
  /\ daemon = "live"
  /\ child = "none"
  /\ child' = "forked"
  /\ references' = references + 1
  /\ UNCHANGED <<daemon, outcome, coordinators, wronglyRefused>>

\* `exec`: with CLOEXEC the child's copy goes away (`std` sets it); the
\* counterfactual keeps the reference, and a long tool child then holds the lock
\* with no coordinator behind it.
ExecChild ==
  /\ child = "forked"
  /\ child' = "execd"
  /\ references' = IF ChildKeepsLock THEN references ELSE references - 1
  /\ UNCHANGED <<daemon, outcome, coordinators, wronglyRefused>>

\* The tool child exits (however long the command ran).
ChildExits ==
  /\ child = "execd"
  /\ child' = "none"
  /\ references' = IF ChildKeepsLock THEN references - 1 ELSE references
  /\ UNCHANGED <<daemon, outcome, coordinators, wronglyRefused>>

\* The coordinator crashes or is killed: its descriptor goes, and the lock goes
\* with the last reference to it — unless a child still holds a copy.
CrashDaemon ==
  /\ daemon = "live"
  /\ daemon' = "gone"
  /\ references' = references - 1
  /\ coordinators' = coordinators - 1
  /\ UNCHANGED <<child, outcome, wronglyRefused>>

\* A restart lands while a *child's* copy is the only thing holding the lock (the
\* fork window after the coordinator died): wait for exec, which is what the
\* counterfactual does not do.
RestartInTheWindow ==
  /\ daemon = "gone"
  /\ child = "forked"
  /\ references > 0
  /\ ReportInsteadOfWaiting
  /\ outcome' = "refused-window"
  /\ wronglyRefused' = TRUE
  /\ UNCHANGED <<daemon, child, references, coordinators>>

WaitForTheWindow ==
  /\ daemon = "gone"
  /\ child = "forked"
  /\ references > 0
  /\ ~ReportInsteadOfWaiting
  /\ outcome' = "waited"
  /\ UNCHANGED <<daemon, child, references, coordinators, wronglyRefused>>

Stutter == UNCHANGED monVars

Next ==
  \/ AcquireFree
  \/ AcquireHeldByDaemon
  \/ AcquireAnyway
  \/ ForkChild
  \/ ExecChild
  \/ ChildExits
  \/ CrashDaemon
  \/ RestartInTheWindow
  \/ WaitForTheWindow
  \/ Stutter

Init ==
  /\ daemon = "gone"
  /\ child = "none"
  /\ references = 0
  /\ outcome = "none"
  /\ coordinators = 0
  /\ wronglyRefused = FALSE

\* Weak fairness on taking a free lock: the restart is retried at the code's poll
\* pace, so a free lock does not stay free.
Spec == Init /\ [][Next]_monVars /\ WF_monVars(AcquireFree)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ daemon \in {"live", "gone"}
  /\ child \in ChildPhases
  /\ references \in 0..2
  /\ outcome \in Outcomes
  /\ coordinators \in 0..2
  /\ wronglyRefused \in BOOLEAN

\* §6.1: one coordinator per state root — the kernel grants the lock to exactly
\* one holder.
AtMostOneCoordinator == coordinators <= 1

\* A33: the fork window is waited out, never mistaken for a coordinator that is
\* not really there.
TheWindowIsNotMistakenForAHolder == wronglyRefused = FALSE

\* A33's other half: with no coordinator alive, the only thing that may still
\* hold the lock is a child in its fork window — nothing inherits it past exec.
NoHeldLockWithoutAHolder == (references > 0 /\ daemon = "gone") => child = "forked"

\* --------------------------------------------------------------- properties --
\* §6.1: free it and it is taken (the retry is the code's poll loop).
RecoveryIsLive == [](references = 0 /\ daemon = "gone" => <>(daemon = "live"))

=============================================================================
