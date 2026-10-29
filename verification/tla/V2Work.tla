------------------------------ MODULE V2Work ---------------------------------
(***************************************************************************
D-487: finite, separate transition scenarios for enrolled durable work.
Each Init chooses one scenario. This checks local contracts, not their full
Cartesian composition or a refinement proof of the Rust/SQLite implementation.
Code: core/src/v2/control/work.rs, work/admission.rs, work/takeover.rs;
engine/src/v2/driver.rs. Kernel classification and process identity need tests.
A response counter counts completed unproductive responses, not lost requests.
Neither model judgement, eventual semantic completion, process descendants,
external MCP effects, fork I/O nor real token prices are modeled here.
Bad selects one plausible missing guard. Every behavioral claim has a control.
***************************************************************************)
EXTENDS Naturals
CONSTANT Bad
VARIABLES
  s
vars == <<s>>
Scenarios == {"recovery", "stop", "attention", "takeover", "scope", "admission"}
RecoveryInit == [responses |-> 0, repairs |-> 0, phase |-> "READY", pending |-> TRUE]
AdmissionInit == [known |-> 0, unknown |-> 0, billed |-> 0, unpriced |-> 0,
                  active |-> FALSE, info |-> FALSE, transfers |-> 0, effect |-> FALSE]
Init ==
  \E which \in Scenarios:
    s = [kind |-> which, data |-> CASE which = "recovery" -> RecoveryInit
          [] which = "stop" -> [stopped |-> FALSE, state |-> "RUNNING"]
          [] which = "attention" -> [wait |-> "NONE", task |-> "RUNNING", attention |-> FALSE, answered |-> FALSE]
          [] which = "takeover" -> [retired |-> FALSE, alive |-> TRUE, observed |-> FALSE,
                                      dispatched |-> FALSE, outcome |-> "OUTCOME_UNKNOWN"]
          [] which = "scope" -> [current |-> 0, prepared |-> 3, started |-> 3,
                                   begun |-> <<0, 0>>, completed |-> <<0, 0>>]
          [] which = "admission" -> AdmissionInit]

Request ==
  /\ s.data.phase = "READY" /\ s.data.pending
  /\ s' = [s EXCEPT !.data.phase = "MODEL", !.data.pending = FALSE]
EmptyReply ==
  /\ s.data.phase = "MODEL" /\ s.data.responses < 4
  /\ s' = [s EXCEPT !.data.responses = @ + 1, !.data.repairs = IF @ < 3 THEN @ + 1 ELSE @,
           !.data.phase = IF s.data.repairs >= 2 THEN "BLOCKED" ELSE "READY", !.data.pending = s.data.repairs < 2]
Restart ==
  /\ s' = [s EXCEPT !.data.repairs = IF Bad = "restart" THEN 0 ELSE @,
           !.data.phase = IF @ = "MODEL" THEN "READY" ELSE @,
           !.data.pending = IF s.data.phase = "MODEL" THEN TRUE ELSE @]
NewEvidence ==
  /\ s.data.phase # "MODEL"
  /\ s' = [s EXCEPT !.data = RecoveryInit]
RecoveryNext == Request \/ EmptyReply \/ Restart \/ NewEvidence

Stop == s' = [s EXCEPT !.data.stopped = TRUE, !.data.state = "STOPPED"]
AsyncObservation == s' = [s EXCEPT !.data.state = IF ~s.data.stopped \/ Bad = "stop" THEN "RUNNING" ELSE @]
UserResume == s' = [s EXCEPT !.data.stopped = FALSE, !.data.state = "RUNNING"]
StopNext == Stop \/ AsyncObservation \/ UserResume

Arm ==
  /\ s.data.wait = "NONE"
  /\ s' = [s EXCEPT !.data.wait = IF s.data.attention THEN "CANCELLED" ELSE IF s.data.task = "SUCCEEDED" THEN "SATISFIED" ELSE "PENDING",
           !.data.answered = (s.data.attention \/ s.data.task = "SUCCEEDED") /\ Bad # "unanswered"]
Report ==
  /\ s.data.task = "RUNNING"
  /\ s' = [s EXCEPT !.data.task = "BLOCKED", !.data.attention = TRUE]
Attention ==
  /\ s.data.attention /\ s.data.wait = "PENDING"
  /\ s' = [s EXCEPT !.data.wait = IF Bad = "attention" THEN "SATISFIED" ELSE "CANCELLED",
           !.data.answered = Bad # "unanswered"]
TaskFinish ==
  /\ s.data.task # "SUCCEEDED"
  /\ s' = [s EXCEPT !.data.task = "SUCCEEDED", !.data.wait = IF @ = "PENDING" THEN "SATISFIED" ELSE @,
           !.data.answered = IF s.data.wait = "PENDING" THEN TRUE ELSE @]
AttentionNext == Arm \/ Report \/ Attention \/ TaskFinish

CancelAssignment == s' = [s EXCEPT !.data.retired = TRUE]
ProcessExits == s' = [s EXCEPT !.data.alive = FALSE]
ObserveStop ==
  /\ ~s.data.alive
  /\ s' = [s EXCEPT !.data.observed = TRUE, !.data.outcome = IF Bad = "rewrite" THEN "SUCCEEDED" ELSE @]
Takeover ==
  /\ s.data.retired /\ (s.data.observed \/ Bad = "cancel_stop")
  /\ s' = [s EXCEPT !.data.dispatched = TRUE]
TakeoverNext == CancelAssignment \/ ProcessExits \/ ObserveStop \/ Takeover

ChangeInputOrTask ==
  /\ s.data.current < 2
  /\ s' = [s EXCEPT !.data.current = @ + 1]
Prepare ==
  /\ s.data.started = 3
  /\ s' = [s EXCEPT !.data.prepared = s.data.current]
Begin ==
  /\ s.data.started = 3 /\ s.data.prepared # 3
  /\ (s.data.prepared = s.data.current \/ Bad = "prepare")
  /\ s' = [s EXCEPT !.data.started = s.data.prepared, !.data.prepared = 3, !.data.begun = <<s.data.prepared, s.data.current>>]
Candidate ==
  /\ s.data.started # 3
  /\ s' = [s EXCEPT !.data.started = 3,
           !.data.completed = IF s.data.started = s.data.current \/ Bad = "completion" THEN <<s.data.started, s.data.current>> ELSE @]
ScopeNext == ChangeInputOrTask \/ Prepare \/ Begin \/ Candidate

KnownAttempt ==
  /\ ~s.data.active /\ ~s.data.info /\ s.data.known < 2
  /\ s' = [s EXCEPT !.data.known = @ + 1]
UnpricedAttempt ==
  /\ ~s.data.active /\ ~s.data.info /\ s.data.unknown < 1
  /\ s' = [s EXCEPT !.data.unknown = @ + 1]
InformationAnswer ==
  /\ ~s.data.active /\ ~s.data.info
  /\ s' = [s EXCEPT !.data.info = TRUE, !.data.active = Bad = "info_goal"]
AdmitWork ==
  /\ ~s.data.active /\ ~s.data.info /\ s.data.known + s.data.unknown > 0
  /\ s' = [s EXCEPT !.data.active = TRUE, !.data.transfers = 1, !.data.billed = s.data.known, !.data.unpriced = s.data.unknown]
ReplayAdmission ==
  /\ s.data.active /\ ~s.data.info /\ s.data.transfers < 2
  /\ s' = IF Bad = "double_bill"
          THEN [s EXCEPT !.data.transfers = @ + 1, !.data.billed = @ + s.data.known, !.data.unpriced = @ + s.data.unknown]
          ELSE s
Dispatch ==
  /\ ~s.data.info /\ (s.data.active \/ Bad = "early_effect")
  /\ s' = [s EXCEPT !.data.effect = TRUE]
AdmissionNext == KnownAttempt \/ UnpricedAttempt \/ InformationAnswer \/ AdmitWork \/ ReplayAdmission \/ Dispatch

Next ==
  ((CASE s.kind = "recovery" -> RecoveryNext
           [] s.kind = "stop" -> StopNext
           [] s.kind = "attention" -> AttentionNext
           [] s.kind = "takeover" -> TakeoverNext
           [] s.kind = "scope" -> ScopeNext
           [] s.kind = "admission" -> AdmissionNext)
      \/ UNCHANGED s)
Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ s.kind \in Scenarios
  /\ CASE s.kind = "recovery" -> s.data \in [responses: 0..4, repairs: 0..3, phase: {"READY", "MODEL", "BLOCKED"}, pending: BOOLEAN]
          [] s.kind = "stop" -> s.data \in [stopped: BOOLEAN, state: {"RUNNING", "STOPPED"}]
          [] s.kind = "attention" -> s.data \in [wait: {"NONE", "PENDING", "CANCELLED", "SATISFIED"},
                                             task: {"RUNNING", "BLOCKED", "SUCCEEDED"}, attention: BOOLEAN, answered: BOOLEAN]
          [] s.kind = "takeover" -> s.data \in [retired: BOOLEAN, alive: BOOLEAN, observed: BOOLEAN,
                                            dispatched: BOOLEAN, outcome: {"OUTCOME_UNKNOWN", "SUCCEEDED"}]
          [] s.kind = "scope" -> s.data \in [current: 0..2, prepared: 0..3, started: 0..3,
                                         begun: (0..2) \X (0..2), completed: (0..2) \X (0..2)]
          [] s.kind = "admission" -> s.data \in [known: 0..2, unknown: 0..1, billed: 0..4, unpriced: 0..2,
                                             active: BOOLEAN, info: BOOLEAN, transfers: 0..2, effect: BOOLEAN]
RecoveryRemainsBounded == s.kind = "recovery" => s.data.responses <= 3
StopSurvivesObservation == s.kind = "stop" => (s.data.stopped => s.data.state = "STOPPED")
AttentionIsNotSettlement == s.kind = "attention" => (s.data.wait = "SATISFIED" => s.data.task = "SUCCEEDED")
ResolvedAttentionIsAnswered == s.kind = "attention" => (s.data.wait \in {"CANCELLED", "SATISFIED"} => s.data.answered)
TakeoverRequiresObservation == s.kind = "takeover" => (s.data.dispatched => s.data.observed /\ ~s.data.alive)
UnknownOutcomeIsPreserved == s.kind = "takeover" => s.data.outcome = "OUTCOME_UNKNOWN"
PreparationMatchesCurrentWork == s.kind = "scope" => s.data.begun[1] = s.data.begun[2]
CompletionMatchesCurrentWork == s.kind = "scope" => s.data.completed[1] = s.data.completed[2]
EffectsRequireAdmission == s.kind = "admission" => (s.data.effect => s.data.active)
InformationDoesNotCreateWork == s.kind = "admission" => (s.data.info => ~s.data.active)
AdmissionUsageTransfersOnce == s.kind = "admission" =>
  (s.data.active /\ ~s.data.info => s.data.transfers = 1 /\ s.data.billed = s.data.known /\ s.data.unpriced = s.data.unknown)
=============================================================================
