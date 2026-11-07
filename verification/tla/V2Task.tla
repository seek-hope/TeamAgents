------------------------------ MODULE V2Task ---------------------------------
(***************************************************************************)
(* Task lifecycle, delegation and work settlement (§5.2/§5.3/§5.4/§6.3/§8;    *)
(* acceptance A02, A09, A16).                                               *)
(*                                                                          *)
(* Code anchors: core/src/v2/control.rs — delegate_task (dependency must     *)
(* already exist, assignee must live, the system never delegates, the work    *)
(* must still be ACTIVE), start_task (assignee or user, PENDING -> RUNNING),  *)
(* complete_task (any not-settled status may settle; the narrow return grant   *)
(* dies with SUCCEEDED/FAILED), cancel_task (requester or user; return grant   *)
(* dies), park_tasks_for_unknown (the only task write the system may make:     *)
(* RUNNING -> BLOCKED), set_lifecycle TERMINATED (the user's cascade cancels   *)
(* that instance's open tasks), complete_work (active work + no open           *)
(* operations; it does not look at tasks) and block_work (system, no           *)
(* open-operation check) — both detach the settled work from every instance.   *)
(*                                                                          *)
(* Finding V-G1 (fixed in the code; regression test                          *)
(* `a_settled_work_takes_no_new_work`): a settled work takes no new work.     *)
(*  - delegation requires an ACTIVE work (Delegate);                          *)
(*  - a request bills a work only while it is ACTIVE (ResolvedWork), which is *)
(*    why `NoStaleActiveWork` — no instance points at a settled work — is the *)
(*    invariant that carries the fix;                                        *)
(*  - closing a work detaches it from every instance (CompleteWork/BlockWork).*)
(*                                                                          *)
(* Modelling note: the linearization point for "new work" is the *request*,   *)
(* not the operation. An operation may still register after its work closed   *)
(* when the request was admitted before the close (Request then ImportOp), and *)
(* its usage then settles on that work — honest accounting, not new work.     *)
(*                                                                          *)
(* Counterfactual constants (D-218): each one TRUE is a plausible mistake the  *)
(* invariants must refute, so the module's claims are falsifiable rather than  *)
(* merely stated — the shape V2Jobs/V2Control use. `DelegateToSettledWork`      *)
(* drops the ACTIVE-work gate delegation has (finding V-G1), `BillSettledWork` *)
(* lets a request bill a settled work, `AllowUnorderedDependency` drops both    *)
(* dependency conditions (a prerequisite that exists and is not the task       *)
(* itself), and `LeaveTasksOnTerminate` drops the terminate cascade. The four   *)
(* controls in verification/README.md set exactly one TRUE each.               *)
(*                                                                          *)
(* Known boundary (not enforced by the code, see verification/README.md): a    *)
(* work may close while its own tasks are still open — complete_work checks    *)
(* operations only. Those tasks stay open with a settled work, and the requests *)
(* that keep them moving run without a billing work. A "settled work" therefore *)
(* never means "its tasks are settled"; only delegation and request admission *)
(* are gated.                                                                *)
(*                                                                          *)
(* TLC only accepts temporal formulas that contain actions in the forms       *)
(* <>[]A and []<>A, so "a settled task is never rewritten" and "no new work   *)
(* on a settled work" are checked through monitor variables (below), not       *)
(* through [][A]_v: each step records whether a rewrite or a late registration *)
(* happened, and the invariants assert the record stays empty.                *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Tasks,  \* task slots, e.g. {"t1","t2"}
          Instances,  \* instances, e.g. {"L","W"}
          Works,  \* work_scopes, e.g. {"g1"}
          MaxOps,  \* open-operation bound per work (keeps the state finite)
          DelegateToSettledWork,  \* counterfactual (D-218): delegation ignores the work's status (finding V-G1)
          BillSettledWork,  \* counterfactual: a request bills a work that has already settled
          AllowUnorderedDependency,  \* counterfactual: a prerequisite may be the task itself or a later one
          LeaveTasksOnTerminate,  \* counterfactual: terminating an instance leaves its open tasks open
          SelfDependency,
          StaleActiveWork,
          StrangerWrites,
          ReturnPathClosed,
          RewriteSettled,
          SystemSettles
ASSUME Tasks # {} /\ Instances # {} /\ Works # {} /\ MaxOps > 0

nobody == "none"        \* sentinel owner/actor for not-yet-created tasks
Actors == Instances \cup {"user", "system"}
TaskStatus == {"none", "PENDING", "RUNNING", "BLOCKED", "SUCCEEDED", "FAILED", "CANCELLED"}
Settled == {"SUCCEEDED", "FAILED", "CANCELLED"}
OpenTask == {"PENDING", "RUNNING", "BLOCKED"}
WorkStatus == {"none", "ACTIVE", "SUCCEEDED", "BLOCKED", "FAILED"}
WorkTerminal == {"SUCCEEDED", "BLOCKED", "FAILED"}

VARIABLES
  taskStatus,  \* task -> status
  assignee,    \* task -> the instance that must do the work
  requester,   \* task -> who delegated it (instance or user)
  taskWork,    \* task -> the work it serves
  deps,        \* task -> the tasks it declares as prerequisites
  created,     \* task -> delegation sequence number (the dependency order)
  lastActor,   \* task -> who performed its last write
  returnPath,  \* task -> whether the task_result grant is still live
  workStatus,  \* work -> status
  openOps,     \* work -> open operations (PREPARED/DISPATCH_COMMITTED/RUNNING)
  live,        \* instance -> not terminated
  activeWork,  \* instance -> the work its requests and delegations run under
  requestWork, \* instance -> work of its in-flight request (nogal = no work)
  clock,       \* delegation counter
  rewritten,   \* monitor: tasks whose status changed while settled (must stay {})
  lateTask,    \* monitor: a task was delegated onto a non-active work
  lateRequest  \* monitor: a request resolved to a work that was not ACTIVE

vars == <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
          returnPath, workStatus, openOps, live, activeWork, requestWork, clock>>

nogal == "nogal"    \* sentinel: a request in flight that bills no work

\* ------------------------------------------------------------------- tasks --
\* §5.3 delegation. The prerequisite must already exist, which is exactly what
\* makes the dependency graph acyclic by construction (no cycle can be built
\* out of already-created tasks).
Delegate(t, who, asg, g, ds) ==
  /\ taskStatus[t] = "none"
  /\ who \in Instances \cup {"user"}        \* the system does not delegate
  /\ asg \in Instances /\ live[asg]
  /\ g \in Works /\ (DelegateToSettledWork \/ workStatus[g] = "ACTIVE")   \* a settled work takes no new work
  /\ ds \subseteq Tasks
  /\ (AllowUnorderedDependency \/ (t \notin ds /\ \A d \in ds : taskStatus[d] # "none"))
  /\ taskStatus' = [taskStatus EXCEPT ![t] = "PENDING"]
  /\ assignee' = [assignee EXCEPT ![t] = asg]
  /\ requester' = [requester EXCEPT ![t] = who]   \* the delegator is the requester
  /\ taskWork' = [taskWork EXCEPT ![t] = g]
  /\ deps' = [deps EXCEPT ![t] = ds]
  /\ created' = [created EXCEPT ![t] = clock]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ returnPath' = [returnPath EXCEPT ![t] = "present"]
  /\ clock' = clock + 1
  /\ UNCHANGED <<workStatus, openOps, live, activeWork, requestWork>>

\* §5.3 start: the assignee starts its own task (PENDING -> RUNNING).
Start(t, who) ==
  /\ taskStatus[t] = "PENDING"
  /\ (who = "user" \/ (who = assignee[t] /\ live[who]))
  /\ taskStatus' = [taskStatus EXCEPT ![t] = "RUNNING"]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, returnPath, workStatus, openOps, live,
                activeWork, requestWork, clock>>

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
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, workStatus, openOps, live,
                activeWork, requestWork, clock>>

\* §5.3 cancellation: the requester or the user; the return path dies with it.
Cancel(t, who) ==
  /\ taskStatus[t] \notin Settled /\ taskStatus[t] # "none"
  /\ (who = "user" \/ who = requester[t])
  /\ taskStatus' = [taskStatus EXCEPT ![t] = "CANCELLED"]
  /\ lastActor' = [lastActor EXCEPT ![t] = who]
  /\ returnPath' = [returnPath EXCEPT ![t] = "absent"]
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, workStatus, openOps, live,
                activeWork, requestWork, clock>>

\* §6.3/A09: an operation whose effect cannot be verified parks its instance's
\* running tasks. This is the only task write the system itself may make.
ParkTasks ==
  /\ \E t \in Tasks : taskStatus[t] = "RUNNING"
  /\ taskStatus' = [ t \in Tasks |-> IF taskStatus[t] = "RUNNING" THEN "BLOCKED" ELSE taskStatus[t] ]
  /\ lastActor' = [ t \in Tasks |-> IF taskStatus[t] = "RUNNING" THEN "system" ELSE lastActor[t] ]
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, returnPath, workStatus, openOps, live,
                activeWork, requestWork, clock>>

\* §5.4 termination (the user only: an instance may hold a manage grant for
\* pause/resume, never for termination): the instance's open tasks cancel and
\* the member row stays.
Terminate(i) ==
  /\ i \in Instances /\ live[i]
  /\ live' = [live EXCEPT ![i] = FALSE]
  /\ LET gone == IF LeaveTasksOnTerminate THEN {}
                 ELSE { t \in Tasks : assignee[t] = i /\ taskStatus[t] \in OpenTask } IN
     /\ taskStatus' = [ t \in Tasks |-> IF t \in gone THEN "CANCELLED" ELSE taskStatus[t] ]
     /\ lastActor' = [ t \in Tasks |-> IF t \in gone THEN "user" ELSE lastActor[t] ]
     /\ returnPath' = [ t \in Tasks |-> IF t \in gone THEN "absent" ELSE returnPath[t] ]
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, workStatus, openOps,
                activeWork, requestWork, clock>>

\* ------------------------------------------------------------------- work_scopes --
\* create_work attaches the new work to the instance that will run under it.
CreateWork(g, i) ==
  /\ workStatus[g] = "none"
  /\ workStatus' = [workStatus EXCEPT ![g] = "ACTIVE"]
  /\ activeWork' = [activeWork EXCEPT ![i] = g]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
                returnPath, openOps, live, requestWork, clock>>

\* The work this instance's next request resolves to (budget_work, A18): its own
\* active work while that is ACTIVE, else the work of the *oldest* open task it
\* serves — running work before pending, rowid order in the code — and only while
\* that work is ACTIVE. A settled oldest task therefore leaves the instance
\* unbilled even when a newer task carries a live work, because the code looks at
\* one task and not at the best one. The pick is modelled exactly, by delegation
\* order (`created`): it is what the code does, and it is also what keeps the
\* model symmetric under permuting `Tasks`, since the chosen task is then a
\* function of the state rather than of TLC's internal ordering (D-218).
HasAnOpenTask(i) == \E t \in Tasks : assignee[t] = i /\ taskStatus[t] \in OpenTask

ResolvedWork(i) ==
  IF activeWork[i] \in Works /\ (BillSettledWork \/ workStatus[activeWork[i]] = "ACTIVE")
    THEN activeWork[i]
  ELSE IF HasAnOpenTask(i) THEN
        LET open == { u \in Tasks : assignee[u] = i /\ taskStatus[u] \in OpenTask }
            oldest == CHOOSE t \in open : \A u \in open : created[u] >= created[t]
        IN IF taskWork[oldest] \in Works /\ (BillSettledWork \/ workStatus[taskWork[oldest]] = "ACTIVE")
           THEN taskWork[oldest]
           ELSE "none"
  ELSE "none"

\* begin_request: the request is admitted (and its budget reserved) under that
\* work. A settled work is never a billing target, so a request that resolves
\* to none runs unbudgeted — the same mode a session without a work uses.
Request(i) ==
  /\ requestWork[i] = "none"
  /\ requestWork' = [requestWork EXCEPT ![i] = IF ResolvedWork(i) = "none" THEN nogal ELSE ResolvedWork(i)]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
                returnPath, workStatus, openOps, live, activeWork, clock>>

\* import_response opens the operations of that request against the work the
\* request recorded — whatever the work's status is by now (see the modelling
\* note in the header).
ImportOp(i) ==
  /\ requestWork[i] # "none"
  /\ requestWork[i] \in Works => openOps[requestWork[i]] < MaxOps
  /\ openOps' = [ g \in Works |->
                    IF requestWork[i] = g THEN openOps[g] + 1 ELSE openOps[g] ]
  /\ requestWork' = [requestWork EXCEPT ![i] = "none"]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
                returnPath, workStatus, live, activeWork, clock>>

CloseOp(g) ==
  /\ g \in Works /\ openOps[g] > 0
  /\ openOps' = [openOps EXCEPT ![g] = openOps[g] - 1]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
                returnPath, workStatus, live, activeWork, requestWork, clock>>

\* §8: an active work closes on its stored candidate once its operations are
\* settled. Tasks are not consulted, and the settled work is detached from every
\* instance that pointed at it (finding V-G1) — an in-flight request keeps its
\* recorded work, which is why its operations and usage may still land there.
CompleteWork(g, outcome) ==
  /\ workStatus[g] = "ACTIVE" /\ openOps[g] = 0
  /\ outcome \in {"SUCCEEDED", "FAILED"}
  /\ workStatus' = [workStatus EXCEPT ![g] = outcome]
  /\ activeWork' = [ i \in Instances |-> IF activeWork[i] = g THEN "none" ELSE activeWork[i] ]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
                returnPath, openOps, live, requestWork, clock>>

\* §8: the driver blocks a work when the required checks exhaust their rounds.
\* No open-operation check on this path.
BlockWork(g) ==
  /\ workStatus[g] = "ACTIVE"
  /\ workStatus' = [workStatus EXCEPT ![g] = "BLOCKED"]
  /\ activeWork' = [ i \in Instances |-> IF activeWork[i] = g THEN "none" ELSE activeWork[i] ]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor,
                returnPath, openOps, live, requestWork, clock>>

Stutter == UNCHANGED vars

NextBase ==
  \/ \E t \in Tasks : \E who \in Instances \cup {"user"} : \E asg \in Instances :
     \E g \in Works : \E ds \in SUBSET Tasks :
        Delegate(t, who, asg, g, ds)
  \/ \E t \in Tasks : \E who \in Actors : Start(t, who)
  \/ \E t \in Tasks : \E who \in Actors : \E outcome \in {"SUCCEEDED", "FAILED", "BLOCKED"} :
        Settle(t, who, outcome)
  \/ \E t \in Tasks : \E who \in Actors : Cancel(t, who)
  \/ ParkTasks
  \/ \E i \in Instances : Terminate(i)
  \/ \E g \in Works : \E i \in Instances : CreateWork(g, i)
  \/ \E i \in Instances : Request(i)
  \/ \E i \in Instances : ImportOp(i)
  \/ \E g \in Works : CloseOp(g)
  \/ \E g \in Works : \E outcome \in {"SUCCEEDED", "FAILED"} : CompleteWork(g, outcome)
  \/ \E g \in Works : BlockWork(g)
  \/ Stutter

\* ------------------------------------------------------------- monitors --
\* The three transition facts worth checking are recorded instead of being
\* written as temporal action formulas (which TLC rejects outside <>[]/[]<>).
\* D-462 counterfactuals: each is the mistake itself, one reachable step, disabled unless its constant is set.
SelfDependencyStep ==
  /\ SelfDependency /\ \E t \in Tasks : deps' = [deps EXCEPT ![t] = @ \cup {t}]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, created, lastActor, returnPath, workStatus,
                 openOps, live, activeWork, requestWork, clock, rewritten, lateTask, lateRequest>>

StaleActiveWorkStep ==
  /\ StaleActiveWork
  /\ \E i \in Instances : activeWork[i] \in Works /\ workStatus' = [workStatus EXCEPT ![activeWork[i]] = "SUCCEEDED"]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor, returnPath,
                 openOps, live, activeWork, requestWork, clock, rewritten, lateTask, lateRequest>>

StrangerWritesStep ==
  /\ StrangerWrites /\ \E t \in Tasks, s \in Instances :
        s # assignee[t] /\ s # requester[t] /\ lastActor' = [lastActor EXCEPT ![t] = s]
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, returnPath, workStatus,
                 openOps, live, activeWork, requestWork, clock, rewritten, lateTask, lateRequest>>

ReturnPathClosedStep ==
  /\ ReturnPathClosed
  /\ \E t \in Tasks : returnPath[t] = "present" /\ taskStatus' = [taskStatus EXCEPT ![t] = "SUCCEEDED"]
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, lastActor, returnPath, workStatus,
                 openOps, live, activeWork, requestWork, clock, rewritten, lateTask, lateRequest>>

RewriteSettledStep ==
  /\ RewriteSettled /\ \E t \in Tasks : rewritten' = {t}
  /\ UNCHANGED <<taskStatus, assignee, requester, taskWork, deps, created, lastActor, returnPath,
                 workStatus, openOps, live, activeWork, requestWork, clock, lateTask, lateRequest>>

SystemSettlesStep ==
  /\ SystemSettles
  /\ \E t \in Tasks : lastActor[t] # "system"
       /\ lastActor' = [lastActor EXCEPT ![t] = "system"]
       /\ taskStatus' = [taskStatus EXCEPT ![t] = "SUCCEEDED"]
  /\ UNCHANGED <<assignee, requester, taskWork, deps, created, returnPath, workStatus, openOps, live,
                 activeWork, requestWork, clock, rewritten, lateTask, lateRequest>>

Next ==
  \/ (NextBase
      /\ rewritten' = rewritten \cup { t \in Tasks : taskStatus[t] \in Settled /\ taskStatus'[t] # taskStatus[t] }
      /\ lateTask' = ( lateTask \/ (\E t \in Tasks : taskStatus[t] = "none" /\ taskStatus'[t] = "PENDING"
                          /\ taskWork'[t] \in Works /\ workStatus[taskWork'[t]] # "ACTIVE") )
      /\ lateRequest' = ( lateRequest \/ (\E i \in Instances : requestWork[i] = "none" /\ requestWork'[i] \in Works
                          /\ workStatus[requestWork'[i]] # "ACTIVE") ))
  \/ (\/ SelfDependencyStep \/ StaleActiveWorkStep \/ StrangerWritesStep
      \/ ReturnPathClosedStep \/ RewriteSettledStep \/ SystemSettlesStep)

Init ==
  /\ taskStatus = [ t \in Tasks |-> "none" ]
  /\ assignee = [ t \in Tasks |-> nobody ]
  /\ requester = [ t \in Tasks |-> nobody ]
  /\ taskWork = [ t \in Tasks |-> "none" ]
  /\ deps = [ t \in Tasks |-> {} ]
  /\ created = [ t \in Tasks |-> 0 ]
  /\ lastActor = [ t \in Tasks |-> nobody ]
  /\ returnPath = [ t \in Tasks |-> "absent" ]
  /\ workStatus = [ g \in Works |-> "none" ]
  /\ openOps = [ g \in Works |-> 0 ]
  /\ live = [ i \in Instances |-> TRUE ]
  /\ activeWork = [ i \in Instances |-> "none" ]
  /\ requestWork = [ i \in Instances |-> "none" ]
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
  /\ \A t \in Tasks : taskWork[t] \in Works \cup {"none"}
  /\ \A t \in Tasks : deps[t] \subseteq Tasks
  /\ \A t \in Tasks : lastActor[t] \in Actors \cup {nobody}
  /\ \A t \in Tasks : returnPath[t] \in {"present", "absent"}
  /\ \A g \in Works : workStatus[g] \in WorkStatus
  /\ \A g \in Works : openOps[g] \in 0..MaxOps
  /\ \A g \in Works : openOps[g] \in Nat
  /\ \A i \in Instances : live[i] \in BOOLEAN
  /\ \A i \in Instances : activeWork[i] \in Works \cup {"none"}
  /\ \A i \in Instances : requestWork[i] \in Works \cup {"none", nogal}

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
\* §5.2/§5.3: a settled task is never rewritten ("a terminal state is final"), checked over
\* transitions through the `rewritten` monitor
SettledIsFinal ==
  rewritten = {}

\* finding V-G1: a settled work takes no new work — no instance keeps pointing
\* at a work that has been settled (closing detaches it, and budget_work only
\* resolves ACTIVE work_scopes, so a stale pointer cannot linger)
NoStaleActiveWork ==
  \A i \in Instances : activeWork[i] \in Works => workStatus[activeWork[i]] = "ACTIVE"

\* ... so delegation and request admission never target a settled work
RegisteredWorkNeedsAnActiveWork ==
  lateTask = FALSE

RequestsResolveToActiveWorks ==
  lateRequest = FALSE

=============================================================================