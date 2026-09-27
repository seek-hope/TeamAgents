------------------------------- MODULE V2Retry -------------------------------
(***************************************************************************)
(* The retry budget of one request (A19; D-240 measured `max_retries` ignored, *)
(* D-247 applied it per instance): a transient failure is retried up to the    *)
(* budget, the budget is the *instance's own* profile's value, and when it is  *)
(* exhausted the turn parks with `transient retries exhausted` — never an      *)
(* attempt beyond it, never a park before it, and a budget of 0 means one      *)
(* attempt.                                                                   *)
(*                                                                           *)
(* The rules the code and the documents state:                                *)
(*                                                                           *)
(*   * an instance makes at most `budget + 1` requests for one turn            *)
(*     (`NoAttemptBeyondTheBudget`; `driver.rs` retries while                  *)
(*     `attempt <= self.config.max_retries`, so the budget counts *retries*,   *)
(*     not attempts);                                                         *)
(*   * a budget of 0 is one attempt, not two (`ZeroMeansOneAttempt` — the      *)
(*     off-by-one the wording invites);                                       *)
(*   * the park happens *at* the budget, not before or after it                *)
(*     (`ParkOnlyAfterTheBudget`, A19's "three attempts, then `transient       *)
(*     retries exhausted`" with a budget of 2);                               *)
(*   * the budget each instance runs with is *its own* profile's               *)
(*     (`ResolvedIsTheInstancesOwn`) — D-240's defect was one session          *)
(*     constant serving every member, which the counterfactual here re-creates.*)
(*                                                                           *)
(* Code anchors: engine/src/config.rs (`validate_profiles` refuses a negative  *)
(* budget, D-247), core/src/models.rs (`default_retries`, the budget a config  *)
(* that omits the key gets), engine/src/v2/supervisor.rs (each driver's config *)
(* resolves `providers::resolve_model(&catalog, &profile.model)` and passes    *)
(* that entry's `max_retries`, with the session constant as the fallback),     *)
(* engine/src/v2/driver.rs (`attempt <= max_retries` and the park sentence).   *)
(* The live half is `review/dogfood/max_retries.py` (offline): a config asking *)
(* for 0 sends one request, one asking for 3 sends four, and the key omitted   *)
(* sends the struct default's count.                                          *)
(*                                                                           *)
(* What the model is and is not: two instances with their own budgets and the  *)
(* session constant, attempts counted per turn, and the park. It does not      *)
(* model *which* failures are transient (the classification is               *)
(* `V2Control`'s and the provider edge's), the backoff between attempts, or    *)
(* what the turn does after parking (a goal that parks is `V2Control`'s). Only *)
(* the *budget* half of A19 is here; the report's "provider retry details are  *)
(* outside the model" still holds for the rest. Safety only: whether a park is  *)
(* ever reached is not a liveness property.                                   *)
(*                                                                           *)
(* The three counterfactuals are the defects the rules exist against:          *)
(* `SessionConstantForEveryInstance` (the pre-D-247 shape: one constant serves  *)
(* every member), `OffByOneBudget` (one attempt more than the budget allows)   *)
(* and `ParkWithoutExhaustion` (a park before the budget is spent).            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Instances,      \* the session's instances, e.g. {"ia","ib"}
          Leader,         \* the instance whose profile carries one budget, the rest the other
          LeaderBudget,   \* `[models.<key>] max_retries` of the leader's own profile
          MemberBudget,   \* …and of every other instance's (they may differ — that is D-247)
          SessionConstant,\* the fallback `SupervisorConfig.max_retries` (D-240's value for everyone)
          OffByOneBudget, \* counterfactual: one attempt beyond the budget is allowed
          ParkWithoutExhaustion,  \* counterfactual: the turn parks before the budget is spent
          SessionConstantForEveryInstance  \* counterfactual: every instance uses the session constant

ASSUME Instances # {} /\ Leader \in Instances
    /\ LeaderBudget \in 0..3 /\ MemberBudget \in 0..3 /\ SessionConstant \in 0..3

\* The budget each instance's own profile declares. The *config* file passes three plain values because TLC's
\* configuration parser rejected this model's first attempt at a function value (`[ia |-> 2, ib |-> 0]`, and a
\* tuple before it) — measured while writing this module; the module keeps the mapping, the configuration stays
\* scalar.
Budgets == [i \in Instances |-> IF i = Leader THEN LeaderBudget ELSE MemberBudget]

VARIABLES
  built,     \* instances whose driver exists (the budget is resolved when it is built)
  budget,    \* instance -> the budget its driver was built with
  attempts,  \* instance -> requests made for the current turn
  parked     \* instance -> the turn gave up and parked

vars == <<built, budget, attempts, parked>>

\* ------------------------------------------------------------------- actions --
\* A driver is built: it resolves its instance's own profile and takes that budget. The fallback the supervisor
\* keeps for a profile the catalog cannot resolve is a session constant; the counterfactual gives *every*
\* instance the constant, which is what D-240 measured.
BuildDriver(i) ==
  /\ i \notin built
  /\ built' = built \cup {i}
  /\ budget' = [budget EXCEPT ![i] = IF SessionConstantForEveryInstance THEN SessionConstant ELSE Budgets[i]]
  /\ UNCHANGED <<attempts, parked>>

\* One request that fails transiently. It is *sent*, so the count moves first; whether it may be retried is the
\* guard on the next request, and the off-by-one counterfactual relaxes that guard by one.
Attempt(i) ==
  /\ i \in built
  /\ attempts[i] < budget[i] + 1 + (IF OffByOneBudget THEN 1 ELSE 0)
  /\ attempts' = [attempts EXCEPT ![i] = attempts[i] + 1]
  /\ UNCHANGED <<built, budget, parked>>

\* The budget is gone: the turn parks with `transient retries exhausted`. The counterfactual parks early, which is
\* the shape a user would notice as "it gave up although retries were left".
Park(i) ==
  /\ i \in built
  /\ ~parked[i]
  /\ (ParkWithoutExhaustion \/ attempts[i] = budget[i] + 1)
  /\ parked' = [parked EXCEPT ![i] = TRUE]
  /\ UNCHANGED <<built, budget, attempts>>

\* An idle step keeps the model open-ended (TLC then reports no false deadlock).
Stutter == UNCHANGED vars

Next ==
  \/ \E i \in Instances : BuildDriver(i)
  \/ \E i \in Instances : Attempt(i)
  \/ \E i \in Instances : Park(i)
  \/ Stutter

Init ==
  /\ built = {}
  /\ budget = [i \in Instances |-> 0]
  /\ attempts = [i \in Instances |-> 0]
  /\ parked = [i \in Instances |-> FALSE]

Spec == Init /\ [][Next]_vars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ built \subseteq Instances
  /\ budget \in [Instances -> 0..3]
  /\ attempts \in [Instances -> 0..5]
  /\ parked \in [Instances -> BOOLEAN]

\* A19: at most the first attempt plus `budget` retries, for every instance.
NoAttemptBeyondTheBudget == \A i \in Instances : attempts[i] <= budget[i] + 1

\* The wording invites an off-by-one; a budget of 0 is one attempt, not two.
ZeroMeansOneAttempt == \A i \in Instances : budget[i] = 0 => attempts[i] <= 1

\* A19: exhaustion parks the turn — and only exhaustion does.
ParkOnlyAfterTheBudget == \A i \in Instances : parked[i] => attempts[i] = budget[i] + 1

\* D-247: the budget an instance runs with is its own profile's, not the session's
\* (`MC_retry_session_constant.cfg` is the D-240 shape that breaks this).
ResolvedIsTheInstancesOwn == \A i \in built : budget[i] = Budgets[i]

=============================================================================
