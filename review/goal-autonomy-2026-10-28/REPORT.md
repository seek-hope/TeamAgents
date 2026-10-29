# Goal autonomy and prompt redesign: source study and proposal

Date: 2026-10-28. **Dated research and original proposal**, subsequently approved and implemented as
[D-487](../../docs/DECISIONS.md#d-487-persistent-work-context-and-bounded-autonomous-continuation-2026-10-28).
TeamAgents source inspected here: `19ac5c8915dc61998a1218ab722c8e7ebc9b0ba8` (D-486).
Current behavior and evidence are in DECISIONS and ACCEPTANCE; the remaining candidate queue is
[PRODUCT-COMPARISON](../../docs/PRODUCT-COMPARISON.md). The proposal below is not the current runtime contract.

## Scope and evidence

The requested direction is a smaller behavioral prompt, clear and natural communication, enough interaction
before execution to settle material intent, and goals that support sustained autonomous work. This study
covers execution loops, durable progress, context renewal, collaboration, stopping and recovery, regardless
of whether an upstream project calls its mechanism a goal.

Seven public repositories were pinned through GitHub's API. [sources.json](sources.json) records full commits,
retrieved paths and SHA-256 hashes. The mechanisms below were inspected in source; this is a targeted study,
not a full audit of any upstream repository. No upstream agent was run, no paid coding trial was started,
and no comparative success, latency or cost claim follows from this inspection. Search results were discovery
aids. Page extraction was blocked by local DNS resolution to a reserved address, so pinned files were obtained
with the GitHub CLI. Retrieved upstream instructions were treated as material to study, not instructions to run.

The existing TeamAgents CI for the baseline was also checked: run
[40706504751](https://github.com/seek-hope/TeamAgents/actions/runs/40706504751) completed successfully for
`test`, `formal-tla` and `formal-kani`. That result covers D-486, not the proposal below.

## Findings from upstream implementations

### 1. Anthropic autonomous-coding: continuity outside a context window

Pinned commit: `930ad10bb2e21a9ef97fe1a88d2037e18a24465c`.

The [session loop](https://github.com/anthropics/claude-quickstarts/blob/930ad10bb2e21a9ef97fe1a88d2037e18a24465c/autonomous-coding/agent.py#L125)
uses an initializer on a fresh project and a coding prompt on subsequent sessions, creating a fresh client
context for each iteration. The
[coding prompt](https://github.com/anthropics/claude-quickstarts/blob/930ad10bb2e21a9ef97fe1a88d2037e18a24465c/autonomous-coding/prompts/coding_prompt.md)
asks the agent to inspect Git history, a progress file and a feature list, check existing behavior, implement
one feature and leave a useful handoff.

Useful mechanism: a new context can recover the objective, verified work and next step from durable artifacts.
A transcript alone is a poor work ledger. This also supports incremental delivery instead of a single final
write after a long investigation.

Limit: the outer loop is bounded by the optional iteration cap or interruption; it does not independently
stop when every feature passes. The feature flags and requested verification are model-maintained. The
initializer's large fixed feature-list recipe is specific to this demo and should not become a requirement
for ordinary coding tasks. Fresh context on every iteration is also not necessary for TeamAgents.

### 2. Ralph: small work units and a transparent outer loop

Pinned commit: `6c53cb0b831ebe8739c6a003e22af14902d8b0b5`.

The [shell loop](https://github.com/snarktank/ralph/blob/6c53cb0b831ebe8739c6a003e22af14902d8b0b5/ralph.sh#L75)
launches another agent invocation up to an iteration limit, retaining the requirement list, progress notes
and repository work. Its
[prompt](https://github.com/snarktank/ralph/blob/6c53cb0b831ebe8739c6a003e22af14902d8b0b5/prompt.md)
directs one incomplete story at a time, with checks and a progress entry.

Useful mechanism: make the next unit small enough to finish, verify and hand over. An understandable outer
loop can be sufficient; long tasks do not inherently require an elaborate planner hierarchy.

Limit: the script's success condition is the output marker `<promise>COMPLETE</promise>`. It does not
independently validate every requirement when that marker appears. Copying the loop would add persistence
of attempts without proving progress or correctness.

### 3. OpenAI Symphony: task lifetime exceeds one agent run

Pinned commit: `be10a1b79df723d6d7612b5651c8522704dafb2e`.

The [runner](https://github.com/openai/symphony/blob/be10a1b79df723d6d7612b5651c8522704dafb2e/elixir/lib/symphony_elixir/agent_runner.ex#L93)
refreshes issue state between turns and can continue an active issue within its turn bound. The
[orchestrator](https://github.com/openai/symphony/blob/be10a1b79df723d6d7612b5651c8522704dafb2e/elixir/lib/symphony_elixir/orchestrator.ex#L208)
schedules an active-state continuation check after a normal worker exit; failures use a retry path, while
an input-required blocker has its own blocked path. Its reconciliation code checks running issues against
external issue state and detects stalled runs.

Useful mechanism: distinguish a completed agent run from a completed user work item. Reconcile authoritative
work state before continuing, and distinguish continuation, transient failure, unavailable input and completion.

Limit: an active issue is a scheduling signal, not independent proof that another iteration will make useful
progress. Issue state and workpad conventions do not prove code correctness. Its unattended issue-tracker
workflow should not be copied wholesale into an interactive conversation, and its run-level turn bound is
not necessarily a global bound across all retries.

### 4. Magentic-One: revise the plan when execution stalls

Pinned commit: `d07c667a3239ea4ab7af62a53b0f4ff205a1f23d`.

The [orchestrator](https://github.com/microsoft/autogen/blob/d07c667a3239ea4ab7af62a53b0f4ff205a1f23d/python/packages/autogen-agentchat/src/autogen_agentchat/teams/_group_chat/_magentic_one/_magentic_one_orchestrator.py#L318)
asks for a structured progress ledger: whether the request is satisfied, whether progress is occurring,
whether execution is looping, the next instruction and the next speaker. It validates the ledger structure
and speaker, tracks stalls, and re-enters an outer planning loop when the stall threshold is reached.
The same file's save/load methods preserve the message thread, task, facts, plan and progress counters.

Useful mechanism: keep the task model separate from the latest worker conversation, and give the coordinator
a bounded opportunity to change the plan instead of repeating the same instruction.

Limit: satisfaction and progress are model judgments, not independently verified facts. Serialization methods
alone do not establish durable external storage. A planner call at every small step can add unnecessary
latency and model usage. This pinned AutoGen
[README](https://github.com/microsoft/autogen/blob/d07c667a3239ea4ab7af62a53b0f4ff205a1f23d/README.md#L15)
announces archival; the recommendation is to borrow a mechanism, not adopt the framework.

### 5. Deep Agents and LangGraph: recoverable context and explicit persistence

Pinned commits: Deep Agents `cd24d8c52849fdb24e0ac2cab2f909f8df6ba5ca`;
LangGraph `5a3e5dd73982a8ac81fb2c71c15b22ca982838de`.

Deep Agents composes file access, subagents, summarization and optional interruption in its
[agent construction](https://github.com/langchain-ai/deepagents/blob/cd24d8c52849fdb24e0ac2cab2f909f8df6ba5ca/libs/deepagents/deepagents/graph.py#L904).
Its [summarization middleware](https://github.com/langchain-ai/deepagents/blob/cd24d8c52849fdb24e0ac2cab2f909f8df6ba5ca/libs/deepagents/deepagents/middleware/summarization.py#L1)
offloads original conversation history to a backend and retains a summary with retrieval information.
Subagents provide separate contexts instead of making the coordinator consume every intermediate detail.
LangGraph's [checkpoint loop](https://github.com/langchain-ai/langgraph/blob/5a3e5dd73982a8ac81fb2c71c15b22ca982838de/libs/langgraph/langgraph/pregel/_loop.py#L1237)
explicitly controls when checkpoint state is saved, with persistence behavior depending on the configured
checkpointer and durability mode.

Useful mechanism: separate active model context from durable history; preserve pointers to original evidence;
make persistence and resume semantics explicit. A compact handoff should not become the only copy of reality.

Limit: passing through a checkpointer or backend parameter does not establish that every default configuration
is durable. Checkpoints also do not make arbitrary external side effects safe to replay. TeamAgents already
has a transactional store and operation receipts; replacing them with a general graph framework would add
migration cost without addressing the central liveness gap.

### 6. OpenHands SDK: persistence plus bounded detection of repetitive failure

Pinned commit: `ccae18cb897bef7960f210aa3f380b577a84be7c`.

The [conversation implementation](https://github.com/OpenHands/software-agent-sdk/blob/ccae18cb897bef7960f210aa3f380b577a84be7c/openhands-sdk/openhands/sdk/conversation/impl/local_conversation.py#L758)
checks accumulated model cost and supports a single nudge before marking a repeating failure as stuck.
The [detector](https://github.com/OpenHands/software-agent-sdk/blob/ccae18cb897bef7960f210aa3f380b577a84be7c/openhands-sdk/openhands/sdk/conversation/stuck_detector.py#L19)
examines a bounded recent event window for repeated action/observation pairs, repeated errors, monologue,
alternating patterns and context-error loops. The
[persistence example](https://github.com/OpenHands/software-agent-sdk/blob/ccae18cb897bef7960f210aa3f380b577a84be7c/examples/01_standalone_sdk/10_persistence.py)
shows the explicit conversation identity and persistence directory used to resume state.

Useful mechanism: give a failing approach a bounded recovery attempt, retain state, and distinguish an
execution fault from a question only the user can answer.

Limit: repeated operations can be legitimate polling, and a living process can still be unproductive.
These detectors are heuristics, not semantic progress proofs. Persistence does not itself decide which
unfinished user task should be scheduled next.

## Diagnosis of the current TeamAgents baseline

The existing persistence, receipts, grants, native-context handling and daemon are useful foundations. The
proposal addresses orchestration and task continuity; it does not assume that TeamAgents lacks recovery.

1. The [Leader instructions](https://github.com/seek-hope/TeamAgents/blob/19ac5c8915dc61998a1218ab722c8e7ebc9b0ba8/engine/src/v2/daemon.rs#L968)
   require the user to open another goal after settlement. They also mix tool protocol, delegation heuristics
   from one measured workload, and user-facing behavior into one fixed paragraph.
2. The [shared turn predicate](https://github.com/seek-hope/TeamAgents/blob/19ac5c8915dc61998a1218ab722c8e7ebc9b0ba8/engine/src/v2/readiness.rs#L10)
   opens a turn for unaddressed input or work. An already-addressed task is not enough to continue after a
   plain response. The [driver](https://github.com/seek-hope/TeamAgents/blob/19ac5c8915dc61998a1218ab722c8e7ebc9b0ba8/engine/src/v2/driver.rs#L1628)
   documents why: D-65 observed 169 requests and about 1.2 million prompt tokens without progress.
3. The [runtime prompt assembly](https://github.com/seek-hope/TeamAgents/blob/19ac5c8915dc61998a1218ab722c8e7ebc9b0ba8/engine/src/v2/driver.rs#L1344)
   derives the offered tools from grants and bindings, reads instruction files, and appends a settlement rule
   asking an assignee never to end a turn with an open task. Prompt pressure is currently compensating for
   the lack of a useful continuation path after that condition occurs.
4. A plain reply, goal settlement, task settlement and instance scheduling have distinct meanings. Merely
   changing the Leader paragraph cannot repair those boundaries. Conversely, always re-asking an active
   instance would undo the measured protection against a turn storm.
5. A worker can leave work blocked or open while the delegator is waiting for a terminal result. Failure and
   attention must reach the coordinator without being misrepresented as successful completion. Existing
   terminal-task waits and message waits must remain distinguishable.

The earlier DD-11 suggestion, automatically opening the next goal at admitted user input, addresses a
conversation boundary. It does not solve sustained progress within the current goal and should be a small
part of this broader proposal.

## Proposed prompt structure

Use three layers, with one owner for each fact:

| Layer | Contents | Update rule |
|---|---|---|
| Short behavior instructions | Clear communication, material clarification, sustained work within agreed intent, truthful status | Versioned product policy |
| Harness instructions | Tools actually available, task reporting, waiting, completion, evidence retrieval and recovery meanings | Generated from the implemented protocol and offered surface |
| Current work context | Confirmed objective, unresolved decisions, completed criteria, next work, active members, blockers and remaining limits | Projected from authoritative state at a request boundary |

Trusted project instructions retain their existing discovery and authority rules. Retrieved documents and
worker reports are information, not a source of new user authorization. Do not repeat full tool schemas or
all historical design decisions in the behavior prompt. Common policy should reach every member; the Leader
has a small orchestration addition and workers have their assigned scope and reporting contract.

Suggested behavior text, **for the proposed runtime only**, with harness instructions supplied separately:

> Communicate clearly and naturally in the user's language. Lead with the result or next action. Explain
> technical details when they help the user understand or decide. State uncertainty and the evidence behind
> claims.
>
> Before substantive execution, understand the intended outcome, scope, constraints, and what would count
> as done. Inspect available context first. Ask together about unresolved choices that could materially change
> the result. If the request is already clear, proceed without a confirmation ritual. Make routine technical
> decisions yourself and state meaningful assumptions.
>
> Once the scope is clear, carry the agreed work through implementation, verification, and handoff. Preserve
> the user's decisions and existing authorization across turns and context changes. Report useful progress
> without turning it into a request for permission. Ask again only when new information requires a material
> user decision or an operation exceeds the granted scope. Continue independent work while waiting when
> possible.
>
> Keep durable progress and recovery notes through the harness. Distinguish progress, waiting, blocked work,
> and completion. Claim completion only when the agreed outcome is supported by appropriate evidence; report
> remaining gaps honestly.

This is a candidate wording, not a standalone drop-in patch for the current scheduler. The two principal
behavior requirements remain communication and clarification. The final two paragraphs make their execution
consequences explicit. Avoid a broad personality/style rulebook or an unconditional requirement to ask questions.

Clarification should be sufficient for action, not a claim that all future uncertainty can be eliminated:

- Ask before implementation when outcome, compatibility, acceptable data loss, delivery target or an actual
  spending limit has competing interpretations that matter to this request.
- Inspect the repository before asking about facts the agent can discover. Resolving a compiler error or
  choosing a local helper function normally does not need another user decision.
- Group related questions. A direct answer can establish readiness; a separate mandatory approval of a
  rewritten plan would create the interruption the user wants to remove.
- Record the answer once. After compaction or restart, load it instead of asking again. A later user correction
  revises the same work and invalidates affected pending steps; it does not silently replace the whole task.

## Proposed goal and scheduling model

### Keep the existing concepts small

A conversation contains long-lived goals. A goal represents the user's agreed work and stays associated with
its budgets, acceptance policy and authority across many model turns. Delegated tasks remain parts of that
goal. A model turn, a context compaction and a daemon restart are execution boundaries, not new goals.
There is no need to introduce another parallel hierarchy of missions, campaigns and projects.

Use the existing instance phases for execution. Add the minimum persistent work/attention state needed to
express the distinctions below; do not force every distinction into a new top-level goal status. A live goal
can remain ACTIVE while waiting, with an explicit reason. Historical terminal records remain terminal.

| Condition at a safe boundary | Runtime behavior | User-visible meaning |
|---|---|---|
| Required intent is unresolved | Persist one grouped question; admit independent discovery if useful | Needs a decision |
| Agreed work has a runnable next step | Consume a recorded scheduling reason and execute | Working |
| A tool or delegated task is outstanding | Wait for its event or scheduled check | Waiting for work |
| A transient infrastructure failure occurs | Apply bounded retry/backoff to a safe operation | Recovering |
| Work appears stalled | Perform a bounded diagnosis/replan; continue only with a supported next action | Adjusting approach |
| Progress requires a new user decision | Persist the exact blocker and affected scope; continue independent branches | Needs a decision |
| Acceptance evidence is being gathered | Run the applicable checks and inspect artifacts | Verifying |
| The agreed result is established | Settle once and deliver the full answer | Completed |
| User cancellation or an enforced limit applies | Preserve checkpoint and reason; do not mint replacement budget | Stopped |

The exact schema and tool names are implementation choices to finalize with the design. Reuse existing finish,
wait and receipt operations where their meanings fit; a short task should not require a lengthy plan or a
model-authored status object after every tool call. These distinctions must not be presented to users as steps
they have to manage manually.

### Record intent and progress separately from chat

Persist a compact work brief: original request references, intended result, scope, constraints, user decisions,
accepted assumptions and completion criteria. Keep revision and provenance so a model-authored interpretation
cannot silently promote itself to a user instruction. Existing grants and limits remain authoritative; prose
in this brief cannot grant capabilities.

Persist a progress view: current milestone, acceptance items still open, artifacts and receipt references,
active child tasks, blockers, next action and a restart note. Model claims remain distinguishable from
programmatically observed facts and independent verification. A new file or a changed plan is a progress
signal, not proof that a requirement has been met.

Use SQLite and the existing control transaction as the authoritative home. A Markdown export can aid inspection,
but should not be a second writable authority. Scope worker views to their assignment and permissions rather
than exposing all conversation history through a shared work brief.

### Continue with a reason, never just because a goal is ACTIVE

The scheduler needs a persisted, deduplicated reason for another request: admitted input, a tool result, a
child update, a due external check, or an explicit continuation decision for unfinished work. Commit the reason
and its consumption through the control plane, using the goal revision and the processed result boundary to
prevent duplicate scheduling after a crash.

A prose progress response must not silently end authorized work. When the model yields without resolving the
next action, allow a bounded protocol-repair opportunity to select an executable step, an event wait, a real
question or completion. Repeated prose, rewritten plans and self-generated status updates must not create an
unlimited supply of scheduling reasons. The budget for this repair/replan episode must survive restart and
context compaction. Its exact thresholds require fixtures and later measurement; arbitrary global limits on
all useful work would recreate the current friction.

Keep transport retry, repetitive action detection and semantic lack of progress distinct. Use recorded action
and observation patterns as cheap signals; reserve a model-assisted replan for actual uncertainty. Track why
an approach changed. Intentional polling, a running build and a delegated long task must not be labeled stalled
merely because no new file appeared. If no viable next action remains, report an actionable blocker once,
rather than issue more empty model calls or repeatedly ask the user to say "continue".

This deliberately replaces the current *no turn for an addressed task* rule with a narrower *no request without
a valid, bounded scheduling reason* rule. It needs control-plane invariants and tests, not just a prompt edit.

### Collaboration must return usable outcomes

The Leader owns integration and the user-level completion decision. Delegate work with a concrete result,
input references, completion evidence, dependencies and workspace ownership. Give each member enough task
context and the capability grants already authorized for that task. Do not create ambient authority inheritance.

Prefer parallel execution when work is independently executable and integration cost is acceptable. Keep serial
work serial. D-262's existing measured result is evidence about its workload, not a universal rule forbidding
small parallel tasks or requiring a team for every long task.

A member reports artifacts, checks performed, remaining gaps and any blocker. A blocked or idle-unsettled
member creates an attention event for its coordinator, which can repair the assignment, take over, re-dispatch
or seek a user decision. Add this wake path without pretending that BLOCKED is a successful terminal result,
and without silently redefining the existing terminal-task wait contract. Prevent simultaneous takeover and
late worker writes by retiring or fencing the previous assignment before replacement execution. Fencing new
dispatches does not stop an already-running Shell command: before reusing its writable workspace, wait for its
settlement or confirm that its process group has stopped. Task cancellation alone is insufficient. An isolated
workspace and explicit artifact integration can avoid sharing that write surface.

Wake on meaningful outcomes or attention, not every heartbeat. Keep worker detail outside the Leader's active
context unless needed for integration. Give coordination and verification calls honest usage accounting; do not
add a second paid planner for every Shell call.

### Completion, context renewal and recovery

Completion should be a claim checked against the agreed result. Code tasks can use the existing required-check
mechanism plus artifact inspection; research and explanation tasks need appropriate deliverable evidence rather
than invented test commands. Model-authored tests and a green process exit do not independently establish the
user's entire outcome. New model-proposed criteria do not silently become user-required checks.

Preserve native model windows and the existing compaction/readback machinery. Project the current work brief,
confirmed decisions and a concise progress view into the renewed context; retain original evidence by reference.
Do not throw away a healthy context after every small step just because an upstream demo does so.

On restart, reconcile persisted requests, task assignments and operation receipts before scheduling more work.
Reuse known results; preserve OUTCOME_UNKNOWN when an external effect cannot be established. Never replay a
write on the basis of an optimistic summary. A short handoff supplements the journal and cannot override it.

### Subsequent user input

Steering, answers to questions and status inquiries belong to the current work when applicable. A status question
must receive an answer without cancelling the original objective. An explicit new work request after completion
can create a new goal at admission, with recorded lineage and configured policy. Deduplicate it by the admitted
input identity. Do not treat every message as a fresh task, reopen settled records, reset exhausted budgets or
resume a deliberately parked instance implicitly. This is the useful part of the earlier DD-11 proposal.

## Design changes and implementation order

This proposal changes existing confirmed behavior and therefore needs a design decision before runtime changes.
The relevant repository rule is in [AGENTS.md](../../AGENTS.md):
"Any implementation that differs from the confirmed design (even a simpler or better one) must be discussed
with the user and confirmed before it goes into code."

The changes to discuss are concrete: a bounded continuation path after plain text (D-65 and the current
readiness/control invariant), the worker settlement rule, coordinator attention wakeups alongside task waits,
new-goal admission (the current user-opened-goal rule), and versioned prompt application to existing sessions.
This report does not amend those decisions or claim that DD-11 has been accepted.

Suggested delivery order:

1. **Behavior and work brief together.** Separate prompt layers, persist decisions and completion criteria,
   expose the work context to Leader and scoped workers, and add a clear question/wait contract. Define how
   existing stored profiles acquire a policy version without rewriting historical requests. Prompt cleanup
   alone is useful but does not count as completion of DD-11.
2. **Autonomous continuation and stopping.** Add durable continuation reasons, idempotent consumption, bounded
   repair/replan and explicit blockers. Update driver readiness, control validation and diagnostics together.
   Keep the old turn-storm scenario as a required negative test. Include full goal accounting for any newly
   introduced model calls; the known existing reviewer-accounting gap must remain explicit until fixed.
3. **Team outcomes and recovery.** Add coordinator attention wakeups, assignment ownership on takeover,
   persisted handoffs and reconciliation across restart. Keep the existing task-result protocol distinguishable
   from progress and attention. Complete the subsequent-input admission behavior in this same work model.
4. **Product and evaluation.** Render plain-language working/waiting/needs-input/completed states, preserve
   interrupt/detach distinctions, and verify long workflows with scripted providers before a separately scoped
   real-model pilot. No extra permanent sidebar or manual goal-management ceremony is required.

Implementation should remain in Rust, using the current control plane, driver, supervisor, daemon input boundary
and TUI projections. This proposal adds no general workflow framework, OS sandbox, release or external service.

## Validation before claiming long-task support

Start with deterministic scripted providers and temporary state. Tests should cover behavior, not merely the
new schema shape:

- A clear request starts with no redundant question; a materially ambiguous request produces one grouped
  clarification; its answer survives compaction and restart without being asked again.
- A multi-step coding task emits an intermediate prose update, continues to a real next action, verifies the
  result and settles once without requiring another user message.
- Repeated prose, repeated failures and repeated plan edits exhaust a bounded recovery episode. Request count
  and usage stay bounded across driver restart; the D-65 turn storm cannot return.
- A running long command or legitimate poll waits correctly. Retryable transport errors, a genuine blocker,
  a user stop and a completed task follow distinct paths.
- Worker success, failure, BLOCKED and idle-unsettled behavior all reach the coordinator appropriately. A takeover
  cannot race a late write from the previous owner. Integration checks apply to the assembled result.
- Inject crashes before/after progress commit, result receipt and continuation consumption. No work is lost,
  no continuation is duplicated, known effects are reused and unknown effects are not replayed.
- Inject user steering during work, a status question, capability revocation, deadline expiry and budget
  exhaustion. Confirm the latest intent applies at a safe boundary and no replacement goal evades limits.
- Retry and reorder the same admitted input; it creates at most one new goal. A completed historical record
  stays closed. Existing sessions load without silently changing their terminal outcomes or old prompt records.
- Attempt premature completion while agreed work or required evidence is missing. Preserve the claim and report
  the gap; do not accept a completion marker or a green unrelated test as the entire task result.

Update the existing formal model for scheduling/goal/wait invariants where the design changes; old properties
must be explicitly replaced or preserved with an explanation, never weakened merely to pass a configuration.

For a later authorized real-model pilot, use the model's native window and record its source; DeepSeek Flash
uses the confirmed 1M window. Do not manufacture a small real-model context window to force compaction. Compare
the baseline and proposal on held-out multi-stage tasks, including a dependency failure, mid-task correction,
member blockage and restart. Record user decisions, redundant confirmations, abandoned work, unsupported
completion, duplicate/lost effects, elapsed time and all model usage. A longer run is not itself a success.

## Reproducing the source inspection

The manifest distinguishes the pinned TeamAgents baseline from external source commits and lists every retrieved
file. Each external file can be retrieved without executing it:

```bash
gh api 'repos/openai/symphony/contents/elixir/lib/symphony_elixir/agent_runner.ex?ref=be10a1b79df723d6d7612b5651c8522704dafb2e' --jq .content | base64 --decode > /tmp/ta-symphony-agent-runner.ex
sha256sum /tmp/ta-symphony-agent-runner.ex
```

Use the corresponding repository, path and full commit from the manifest for the other files, then compare the
SHA-256 value. The report's source links are pinned to those same commits. Upstream tests were not run; the
validation plan above remains unexecuted until implementation is agreed and built.
