------------------------------ MODULE V2Task ---------------------------------
(***************************************************************************)
(* Task lifecycle, delegation and goal settlement (§5.2/§5.3/§5.4/§6.3/§8;    *)
(* acceptance A02, A09, A16).                                               *)
(*                                                                          *)
(* Code anchors: core/src/v2/control.rs — delegate_task (dependency must     *)
(* already exist, assignee must live, the system never delegates, the goal    *)
(* must still be ACTIVE), start_task (assignee or user, PENDING -> RUNNING),  *)
(* complete_task (any not-settled status may settle; the narrow return grant   *)
(* dies with SUCCEEDED/FAILED), cancel_task (requester or user; return grant   *)
(* dies), park_tasks_for_unknown (the only task write the system may make:     *)
(* RUNNING -> BLOCKED), set_lifecycle TERMINATED (the user's cascade cancels   *)
(* that instance's open tasks), complete_goal (active goal + no open           *)
(* operations; it does not look at tasks) and block_goal (system, no           *)
(* open-operation check) — both detach the settled goal from every instance.   *)
(*                                                                          *)
(* Finding V-G1 (fixed in the code; regression test                          *)
(* `a_settled_goal_takes_no_new_work`): a settled goal takes no new work.     *)
(*  - delegation requires an ACTIVE goal (Delegate);                          *)
(*  - a request bills a goal only while it is ACTIVE (ResolvedGoal), which is *)
(*    why `NoStaleActiveGoal` — no instance points at a settled goal — is the *)
(*    invariant that carries the fix;                                        *)
(*  - closing a goal detaches it from every instance (CompleteGoal/BlockGoal).*)
(*                                                                          *)
(* Modelling note: the linearization point for "new work" is the *request*,   *)
(* not the operation. An operation may still register after its goal closed   *)
(* when the request was admitted before the close (Request then ImportOp), and *)
(* its usage then settles on that goal — honest accounting, not new work.     *)
(*                                                                          *)
(* Known boundary (not enforced by the code, see verification/README.md): a    *)
(* goal may close while its own tasks are still open — complete_goal checks    *)
(* operations only. Those tasks stay open with a settled goal, and the requests *)
(* that keep them moving run without a billing goal. A "settled goal" therefore *)
(* never means "its tasks are settled"; only delegation and request admission *)
(* are gated.                                                                *)
(*                                                                          *)
(* TLC only accepts temporal formulas that contain actions in the forms       *)
(* <>[]A and []<>A, so "a settled task is never rewritten" and "no new work   *)
(* on a settled goal" are checked through monitor variables (below), not       *)
(* through [][A]_v: each step records whether a rewrite or a late registration *)
(* happened, and the invariants assert the record stays empty.                *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Tasks,        \* task slots, e.g. {"t1","t2"}
          Instances,    \* instances, e.g. {"L","W"}
          Goals,        \* goals, e.g. {"g1"}
          MaxOps        \* open-operation bound per goal (keeps the state finite)

ASSUME Tasks # {} /\ Instances # {} /\ Goals # {} /\ MaxOps > 0

nobody == "none"        \* sentinel owner/actor for not-yet-created tasks
Actors == Instances \cup {"user", "system"}
TaskStatus == {"none", "PENDING", "RUNNING", "BLOCKED", "SUCCEEDED", "FAILED", "CANCELLED"}
Settled == {"SUCCEEDED", "FAILED", "CANCELLED"}
OpenTask == {"PENDING", "RUNNING", "BLOCKED"}
GoalStatus == {"none", "ACTIVE", "SUCCEEDED", "BLOCKED", "FAILED"}
GoalTerminal == {"SUCCEEDED", "BLOCKED", "FAILED"}

VARIABLES
  taskStatus,  \* task -> status
  assignee,    \* task -> the instance that must do the work
  requester,   \* task -> who delegated it (instance or user)
  taskGoal,    \* task -> the goal it serves
  deps,        \* task -> the tasks it declares as prerequisites
  created,     \* task -> delegation sequence number (the dependency order)
  lastActor,   \* task -> who performed its last write
  returnPath,  \* task -> whether the task_result grant is still live
  goalStatus,  \* goal -> status
  openOps,     \* goal -> open operations (PREPARED/DISPATCH_COMMITTED/RUNNING)
  live,        \* instance -> not terminated
  activeGoal,  \* instance -> the goal its requests and delegations run under
  requestGoal, \* instance -> goal of its in-flight request (nogal = no goal)
  clock,       \* delegation counter
  rewritten,   \* monitor: tasks whose status changed while settled (must stay {})
  lateTask,    \* monitor: a task was delegated onto a non-active goal
  lateRequest  \* monitor: a request resolved to a goal that was not ACTIVE

vars == <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
          returnPath, goalStatus, openOps, live, activeGoal, requestGoal, clock>>

nogal == "nogal"    \* sentinel: a request in flight that bills no goal

\* ------------------------------------------------------------------- tasks --
\* §5.3 delegation. The prerequisite must already exist, which is exactly what
\* makes the dependency graph acyclic by construction (no cycle can be built
\* out of already-created tasks).
Delegate(t, who, asg, g, ds) ==
  /\ taskStatus[t] = "none"
  /\ who \in Instances \cup {"user"}        \* the system does not delegate
  /\ asg \in Instances /\ live[asg]
  /\ g \in Goals /\ goalStatus[g] = "ACTIVE"   \* a settled goal takes no new work
  /\ ds \subseteq Tasks /\ t \notin ds
  /\ \A d \in ds : taskStatus[d] # "none"
  /\ taskStatus' = [taskStatus EXCEPT ![t] = "PENDING"]
  /\ assignee' = [assignee EXCEPT ![t] = asg]
  /\ requester' = [requester EXCEPT ![t] = who]   \* the delegator is the requester
  /\ taskGoal' = [taskGoal EXCEPT ![t] = g]
  /\ deps' = [deps EXCEPT ![t] = ds]
  /\ created' = [created EXCEPT ![t] = clock]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ returnPath' = [returnPath EXCEPT ![t] = "present"]
  /\ clock' = clock + 1
  /\ UNCHANGED <<goalStatus, openOps, live, activeGoal, requestGoal>>

\* §5.3 start: the assignee starts its own task (PENDING -> RUNNING).
Start(t, who) ==
  /\ taskStatus[t] = "PENDING"
  /\ (who = "user" \/ (who = assignee[t] /\ live[who]))
  /\ taskStatus' = [taskStatus EXCEPT ![t] = "RUNNING"]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ UNCHANGED <<assignee, requester, taskGoal, deps, created, returnPath, goalStatus, openOps, live,
                activeGoal, requestGoal, clock>>

\* §5.3 settlement: the assignee or the user settles any not-settled task
\* (BLOCKED may be settled too, so a parked task is not a dead end). The
\* narrow return capability dies with SUCCEEDED/FAILED.
Settle(t, who, outcome) ==
  /\ taskStatus[t] \notin Settled /\ taskStatus[t] # "none"
  /\ outcome \in {"SUCCEEDED", "FAILED", "BLOCKED"}
  /\ (who = "user" \/ (who = assignee[t] /\ live[who]))
  /\ taskStatus' = [taskStatus EXCEPT ![t] = outcome]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ returnPath' = [returnPath EXCEPT ![t] =
         IF outcome \in {"SUCCEEDED", "FAILED"} THEN "absent" ELSE returnPath[t]]
  /\ UNCHANGED <<assignee, requester, taskGoal, deps, created, goalStatus, openOps, live,
                activeGoal, requestGoal, clock>>

\* §5.3 cancellation: the requester or the user; the return path dies with it.
Cancel(t, who) ==
  /\ taskStatus[t] \notin Settled /\ taskStatus[t] # "none"
  /\ (who = "user" \/ who = requester[t])
  /\ taskStatus' = [taskStatus EXCEPT ![t] = "CANCELLED"]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ returnPath' = [returnPath EXCEPT ![t] = "absent"]
  /\ UNCHANGED <<assignee, requester, taskGoal, deps, created, goalStatus, openOps, live,
                activeGoal, requestGoal, clock>>

\* §6.3/A09: an operation whose effect cannot be verified parks its instance's
\* running tasks. This is the only task write the system itself may make.
ParkTasks ==
  /\ \E t \in Tasks : taskStatus[t] = "RUNNING"
  /\ taskStatus' = [ t \in Tasks |-> IF taskStatus[t] = "RUNNING" THEN "BLOCKED" ELSE taskStatus[t] ]
  /\ lastActor' = [ t \in Tasks |-> IF taskStatus[t] = "RUNNING" THEN "system" ELSE lastActor[t] ]
  /\ UNCHANGED <<assignee, requester, taskGoal, deps, created, returnPath, goalStatus, openOps, live,
                activeGoal, requestGoal, clock>>

\* §5.4 termination (the user only: an instance may hold a manage grant for
\* pause/resume, never for termination): the instance's open tasks cancel and
\* the member row stays.
Terminate(i) ==
  /\ i \in Instances /\ live[i]
  /\ live' = [live EXCEPT ![i] = FALSE]
  /\ LET gone == { t \in Tasks : assignee[t] = i /\ taskStatus[t] \in OpenTask } IN
     /\ taskStatus' = [ t \in Tasks |-> IF t \in gone THEN "CANCELLED" ELSE taskStatus[t] ]
     /\ lastActor' = [ t \in Tasks |-> IF t \in gone THEN "user" ELSE lastActor[t] ]
     /\ returnPath' = [ t \in Tasks |-> IF t \in gone THEN "absent" ELSE returnPath[t] ]
  /\ UNCHANGED <<assignee, requester, taskGoal, deps, created, goalStatus, openOps,
                activeGoal, requestGoal, clock>>

\* ------------------------------------------------------------------- goals --
\* create_goal attaches the new goal to the instance that will run under it.
CreateGoal(g, i) ==
  /\ goalStatus[g] = "none"
  /\ goalStatus' = [goalStatus EXCEPT ![g] = "ACTIVE"]
  /\ activeGoal' = [activeGoal EXCEPT ![i] = g]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
                returnPath, openOps, live, requestGoal, clock>>

\* The goal this instance's next request resolves to (budget_goal, A18): its own
\* active goal while that is ACTIVE, else the goal of the single oldest open
\* task it serves — running work before pending, rowid order in the code — and
\* only while *that* goal is ACTIVE. A settled oldest task therefore makes the
\* instance run unbilled even when a newer task carries a live goal: the code
\* looks at one task, not at the best one.
\* The goal this instance's next request resolves to (budget_goal, A18): its own
\* active goal while that is ACTIVE, else the goal of an open task it serves —
\* and only while *that* goal is ACTIVE. A settled goal is never a billing
\* target, whatever the fallback picks.
\* (Modelled loosely on purpose: the code consults the single oldest open task,
\* so a stale oldest task leaves the instance unbilled even when a newer task
\* carries a live goal. The gate is identical either way, so the model keeps the
\* gate and records the pick-order nuance in the header note.)
TaskGoalOf(i) == taskGoal[CHOOSE t \in Tasks : assignee[t] = i /\ taskStatus[t] \in OpenTask]

HasAnOpenTask(i) == \E t \in Tasks : assignee[t] = i /\ taskStatus[t] \in OpenTask

ResolvedGoal(i) ==
  IF activeGoal[i] \in Goals /\ goalStatus[activeGoal[i]] = "ACTIVE" THEN activeGoal[i]
  ELSE IF HasAnOpenTask(i) /\ TaskGoalOf(i) \in Goals
          /\ goalStatus[TaskGoalOf(i)] = "ACTIVE"
       THEN TaskGoalOf(i)
  ELSE "none"

\* begin_request: the request is admitted (and its budget reserved) under that
\* goal. A settled goal is never a billing target, so a request that resolves
\* to none runs unbudgeted — the same mode a session without a goal uses.
Request(i) ==
  /\ requestGoal[i] = "none"
  /\ requestGoal' = [requestGoal EXCEPT ![i] = IF ResolvedGoal(i) = "none" THEN nogal ELSE ResolvedGoal(i)]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
                returnPath, goalStatus, openOps, live, activeGoal, clock>>

\* import_response opens the operations of that request against the goal the
\* request recorded — whatever the goal's status is by now (see the modelling
\* note in the header).
ImportOp(i) ==
  /\ requestGoal[i] # "none"
  /\ requestGoal[i] \in Goals => openOps[requestGoal[i]] < MaxOps
  /\ openOps' = [ g \in Goals |->
                    IF requestGoal[i] = g THEN openOps[g] + 1 ELSE openOps[g] ]
  /\ requestGoal' = [requestGoal EXCEPT ![i] = "none"]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
                returnPath, goalStatus, live, activeGoal, clock>>

CloseOp(g) ==
  /\ g \in Goals /\ openOps[g] > 0
  /\ openOps' = [openOps EXCEPT ![g] = openOps[g] - 1]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
                returnPath, goalStatus, live, activeGoal, requestGoal, clock>>

\* §8: an active goal closes on its stored candidate once its operations are
\* settled. Tasks are not consulted, and the settled goal is detached from every
\* instance that pointed at it (finding V-G1) — an in-flight request keeps its
\* recorded goal, which is why its operations and usage may still land there.
CompleteGoal(g, outcome) ==
  /\ goalStatus[g] = "ACTIVE" /\ openOps[g] = 0
  /\ outcome \in {"SUCCEEDED", "FAILED"}
  /\ goalStatus' = [goalStatus EXCEPT ![g] = outcome]
  /\ activeGoal' = [ i \in Instances |-> IF activeGoal[i] = g THEN "none" ELSE activeGoal[i] ]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
                returnPath, openOps, live, requestGoal, clock>>

\* §8: the driver blocks a goal when the required checks exhaust their rounds.
\* No open-operation check on this path.
BlockGoal(g) ==
  /\ goalStatus[g] = "ACTIVE"
  /\ goalStatus' = [goalStatus EXCEPT ![g] = "BLOCKED"]
  /\ activeGoal' = [ i \in Instances |-> IF activeGoal[i] = g THEN "none" ELSE activeGoal[i] ]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskGoal, deps, created, lastActor,
                returnPath, openOps, live, requestGoal, clock>>

Stutter == UNCHANGED vars

NextBase ==
  \/ \E t \in Tasks : \E who \in Instances \cup {"user"} : \E asg \in Instances :
     \E g \in Goals : \E ds \in SUBSET Tasks :
        Delegate(t, who, asg, g, ds)
  \/ \E t \in Tasks : \E who \in Actors : Start(t, who)
  \/ \E t \in Tasks : \E who \in Actors : \E outcome \in {"SUCCEEDED", "FAILED", "BLOCKED"} :
        Settle(t, who, outcome)
  \/ \E t \in Tasks : \E who \in Actors : Cancel(t, who)
  \/ ParkTasks
  \/ \E i \in Instances : Terminate(i)
  \/ \E g \in Goals : \E i \in Instances : CreateGoal(g, i)
  \/ \E i \in Instances : Request(i)
  \/ \E i \in Instances : ImportOp(i)
  \/ \E g \in Goals : CloseOp(g)
  \/ \E g \in Goals : \E outcome \in {"SUCCEEDED", "FAILED"} : CompleteGoal(g, outcome)
  \/ \E g \in Goals : BlockGoal(g)
  \/ Stutter

\* ------------------------------------------------------------- monitors --
\* The three transition facts worth checking are recorded instead of being
\* written as temporal action formulas (which TLC rejects outside <>[]/[]<>).
Next ==
  /\ NextBase
  /\ rewritten' = rewritten \cup { t \in Tasks : taskStatus[t] \in Settled /\ taskStatus'[t] # taskStatus[t] }
  /\ lateTask' = ( lateTask \/
        (\E t \in Tasks : taskStatus[t] = "none" /\ taskStatus'[t] = "PENDING"
                          /\ taskGoal'[t] \in Goals
                          /\ goalStatus[taskGoal'[t]] # "ACTIVE") )
  /\ lateRequest' = ( lateRequest \/
        (\E i \in Instances : requestGoal[i] = "none" /\ requestGoal'[i] \in Goals
                              /\ goalStatus[requestGoal'[i]] # "ACTIVE") )

Init ==
  /\ taskStatus = [ t \in Tasks |-> "none" ]
  /\ assignee = [ t \in Tasks |-> nobody ]
  /\ requester = [ t \in Tasks |-> nobody ]
  /\ taskGoal = [ t \in Tasks |-> "none" ]
  /\ deps = [ t \in Tasks |-> {} ]
  /\ created = [ t \in Tasks |-> 0 ]
  /\ lastActor = [ t \in Tasks |-> nobody ]
  /\ returnPath = [ t \in Tasks |-> "absent" ]
  /\ goalStatus = [ g \in Goals |-> "none" ]
  /\ openOps = [ g \in Goals |-> 0 ]
  /\ live = [ i \in Instances |-> TRUE ]
  /\ activeGoal = [ i \in Instances |-> "none" ]
  /\ requestGoal = [ i \in Instances |-> "none" ]
  /\ clock = 0
  /\ rewritten = {}
  /\ lateTask = FALSE
  /\ lateRequest = FALSE

\* the monitors are part of the next-state relation's subscript: every step
\* that Next can take assigns them, so no warning about unconstrained variables
monVars == <<vars, rewritten, lateTask, lateRequest>>

Spec == Init /\ [][Next]_monVars

\* --------------------------------------------------------------- invariants --
TypeOK ==
  /\ \A t \in Tasks : taskStatus[t] \in TaskStatus
  /\ \A t \in Tasks : assignee[t] \in Instances \cup {nobody}
  /\ \A t \in Tasks : requester[t] \in Instances \cup {"user", nobody}
  /\ \A t \in Tasks : taskGoal[t] \in Goals \cup {"none"}
  /\ \A t \in Tasks : deps[t] \subseteq Tasks
  /\ \A t \in Tasks : lastActor[t] \in Actors \cup {nobody}
  /\ \A t \in Tasks : returnPath[t] \in {"present", "absent"}
  /\ \A g \in Goals : goalStatus[g] \in GoalStatus
  /\ \A g \in Goals : openOps[g] \in 0..MaxOps
  /\ \A g \in Goals : openOps[g] \in Nat
  /\ \A i \in Instances : live[i] \in BOOLEAN
  /\ \A i \in Instances : activeGoal[i] \in Goals \cup {"none"}
  /\ \A i \in Instances : requestGoal[i] \in Goals \cup {"none", nogal}

\* the system never settles or cancels: its only task write is parking to BLOCKED
SystemOnlyParksTasks ==
  \A t \in Tasks : lastActor[t] = "system" => taskStatus[t] = "BLOCKED"

\* only a party to the task writes it: the assignee, the requester, the user,
\* or the system parking it (never an unrelated instance)
OnlyPartiesWriteTasks ==
  \A t \in Tasks : lastActor[t] \in {"user", "system", nobody, assignee[t], requester[t]}

\* a live return path exists exactly while the task is open — the narrow
\* capability is revoked with SUCCEEDED/FAILED (a BLOCKED task keeps it, so it
\* can still report its result)
ReturnPathOnlyWhileOpen ==
  \A t \in Tasks : returnPath[t] = "present" => taskStatus[t] \in OpenTask

\* dependency edges always point at an older task, so the declared dependency
\* graph is acyclic by construction (the code refuses unknown or self
\* prerequisites, and an unknown task cannot be an older one)
DependenciesPointBackwards ==
  \A t \in Tasks : \A d \in deps[t] : created[d] < created[t]

NoSelfDependency ==
  \A t \in Tasks : t \notin deps[t]

\* §5.4: terminating an instance leaves no open task of its own behind
NoOpenTaskOnDeadAssignee ==
  \A t \in Tasks : assignee[t] \in Instances => (live[assignee[t]] \/ taskStatus[t] \notin OpenTask)

\* ------------------------------------------------------------------ properties --
\* §5.2/§5.3: a settled task is never rewritten ("终态不可改写"), checked over
\* transitions through the `rewritten` monitor
SettledIsFinal ==
  rewritten = {}

\* finding V-G1: a settled goal takes no new work — no instance keeps pointing
\* at a goal that has been settled (closing detaches it, and budget_goal only
\* resolves ACTIVE goals, so a stale pointer cannot linger)
NoStaleActiveGoal ==
  \A i \in Instances : activeGoal[i] \in Goals => goalStatus[activeGoal[i]] = "ACTIVE"

\* ... so delegation and request admission never target a settled goal
RegisteredWorkNeedsAnActiveGoal ==
  lateTask = FALSE

RequestsResolveToActiveGoals ==
  lateRequest = FALSE

=============================================================================
