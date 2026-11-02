------------------------- MODULE V2JobSupervision -------------------------
(***************************************************************************)
(* D-494: bounded runner supervision beside arbitrary socket traffic.       *)
(* Anchors: engine/src/jobs/runner.rs request/tick/run/serve_client.          *)
(* This complements V2Jobs' durable start/recovery handshake; it does not    *)
(* compose the state spaces. One clock step abstracts a deadline/grace tick,*)
(* signal delivery is abstract, and the owned leader anchors group identity.*)
(* No claim covers escaped descendants, remote effects, OS fairness, disk   *)
(* failures, process-id reuse, TCP, byte parsing or Rust/SQLite refinement.  *)
(* Clients may stay open or send traffic forever. Only the clock, supervisor*)
(* and retirement steps are weakly fair; client cooperation is not assumed. *)
(***************************************************************************)
EXTENDS Naturals
CONSTANT Bug
VARIABLES
  s
vars == <<s>>
Modes == {"deadline", "cancel", "prestart", "recovery", "normal", "write-failure", "completion-race"}
Terminal == {"CANCELLED", "CANCELLED_BEFORE_START", "OUTCOME_UNKNOWN", "SUCCEEDED", "FAILED"}

Init ==
  \E mode \in Modes :
    s = [mode |-> mode, runner |-> TRUE, socket |-> FALSE, due |-> FALSE,
         expired |-> FALSE, phase |-> IF mode \in {"prestart", "write-failure"} THEN "READY"
                                      ELSE IF mode = "recovery" THEN "OUTCOME_UNKNOWN" ELSE "RUNNING",
         leaderExited |-> mode = "completion-race", ownsLeader |-> mode \notin {"prestart", "recovery", "write-failure"},
         descendant |-> mode \notin {"prestart", "recovery", "write-failure"}, killSent |-> FALSE,
         accepted |-> mode \in {"deadline", "cancel", "normal", "completion-race"},
         started |-> mode \in {"deadline", "cancel", "normal", "completion-race"},
         age |-> 0, graceReset |-> FALSE,
         timestamp |-> mode = "recovery" /\ Bug # "missing-time"]

Connect == /\ s.runner /\ ~s.socket /\ s' = [s EXCEPT !.socket = TRUE]
Disconnect == /\ s.socket /\ s' = [s EXCEPT !.socket = FALSE]
Traffic == /\ s.runner
           /\ s' = [s EXCEPT !.due = IF Bug = "reset-timer" THEN FALSE ELSE @]
Clock == /\ s.runner
         /\ s' = [s EXCEPT !.expired = TRUE, !.due = TRUE,
                           !.age = IF s.phase = "CANCEL_REQUESTED" THEN 1 ELSE @]

Cancel ==
  /\ s.runner
  /\ s.mode \in {"cancel", "prestart"}
  /\ s.phase \in {"READY", "RUNNING", "CANCEL_REQUESTED"}
  /\ IF s.phase = "READY"
     THEN s' = [s EXCEPT !.phase = "CANCELLED_BEFORE_START", !.timestamp = Bug # "missing-time"]
     ELSE IF s.phase = "RUNNING"
     THEN s' = [s EXCEPT !.phase = "CANCEL_REQUESTED", !.age = 0]
     ELSE s' = [s EXCEPT !.age = IF Bug = "reset-grace" THEN 0 ELSE @,
                        !.graceReset = @ \/ (Bug = "reset-grace" /\ s.age = 1)]

\* A failed acceptance write cannot launch anything. This is separate from
\* V2Jobs' assumption that its AcceptGo step successfully persists.
FailedAcceptance ==
  /\ s.runner /\ s.mode = "write-failure" /\ s.phase = "READY"
  /\ s' = [s EXCEPT !.phase = "FAILED", !.timestamp = TRUE,
                    !.started = (Bug = "spawn-unsaved")]

LeaderExit ==
  /\ s.runner /\ s.ownsLeader /\ ~s.leaderExited
  /\ (s.phase = "CANCEL_REQUESTED" \/ s.mode = "normal")
  /\ s' = [s EXCEPT !.leaderExited = TRUE]

Poll ==
  /\ s.runner /\ s.due
  /\ ~(Bug = "socket-blocks" /\ s.socket)
  /\ IF s.phase = "RUNNING" /\ s.expired
        /\ (s.mode = "deadline" \/ (s.mode = "completion-race" /\ Bug = "deadline-first"))
     THEN s' = [s EXCEPT !.phase = "CANCEL_REQUESTED", !.age = 0, !.due = FALSE]
     ELSE IF s.phase = "CANCEL_REQUESTED" /\ s.leaderExited /\ Bug = "early-reap"
     THEN s' = [s EXCEPT !.phase = "CANCELLED", !.ownsLeader = FALSE, !.timestamp = TRUE, !.due = FALSE]
     ELSE IF s.phase = "CANCEL_REQUESTED" /\ s.age = 1
     THEN s' = [s EXCEPT !.phase = "CANCELLED", !.killSent = TRUE,
                        !.descendant = FALSE, !.ownsLeader = FALSE, !.timestamp = TRUE, !.due = FALSE]
     ELSE IF s.phase = "RUNNING" /\ s.mode \in {"normal", "completion-race"} /\ s.leaderExited
     THEN s' = [s EXCEPT !.phase = "SUCCEEDED", !.ownsLeader = FALSE,
                        !.killSent = Bug = "kill-normal", !.descendant = Bug # "kill-normal",
                        !.timestamp = TRUE, !.due = FALSE]
     ELSE s' = [s EXCEPT !.due = FALSE]

Retire == /\ s.runner /\ s.phase \in Terminal /\ s.timestamp
          /\ s' = [s EXCEPT !.runner = FALSE]
Next == Connect \/ Disconnect \/ Traffic \/ Clock \/ Cancel \/ FailedAcceptance \/ LeaderExit \/ Poll \/ Retire \/ UNCHANGED vars
Spec == Init /\ [][Next]_vars /\ WF_vars(Clock) /\ WF_vars(Poll) /\ WF_vars(Retire)

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ s.mode \in Modes
  /\ s.phase \in Terminal \cup {"READY", "RUNNING", "CANCEL_REQUESTED"}
  /\ s.age \in 0..1
  /\ \A field \in {"runner", "socket", "due", "expired", "leaderExited", "ownsLeader",
                   "descendant", "killSent", "graceReset", "timestamp", "accepted", "started"} : s[field] \in BOOLEAN
UnsavedAcceptanceNeverExecutes == s.started => s.accepted
CancelGraceNeverReplenished == ~s.graceReset
CancelledGroupWasEscalated == s.phase = "CANCELLED" => s.killSent
TerminalHasRetirementTime == s.phase \in Terminal => s.timestamp
NormalServicesArePreserved == s.mode \in {"normal", "completion-race"} => ~s.killSent

\* --------------------------------------------------------------- properties --
DeadlineEventuallyCancels == (s.mode = "deadline" /\ s.expired) ~> (s.phase = "CANCELLED")
SettledRunnerEventuallyLeaves == (s.phase \in Terminal) ~> ~s.runner
=============================================================================
