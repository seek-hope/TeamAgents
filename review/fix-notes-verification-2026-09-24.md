# Fix ledger: two issues found by formal verification (2026-09-24)

This round started with the user asking for the design to be checked with formal methods and then confirming
"fix everything". The TLA+/TLC specs produced counterexamples first, code probes confirmed them, and the fixes
landed with regression tests. The property mapping is in [verification/README.md](../verification/README.md)
and the decision record is D-44 in `docs/DECISIONS.md`.

Re-run commands: `make verify-model-all` (exhaustive configurations for the modules) and `make check`
(formatting, static checks, all tests).

## 1. V-W1: a wait's tool_call was not answered on the two non-drain exits

- **Property**: `ResolvedWaitIsAnswered` (before the fix: `ResolvedWaitAnswersItsCall` in the
  expected-counterexample configuration `MC_wait_contract.cfg`).
- **Spec counterexample** (re-runnable before the fix): `Error: Invariant ResolvedWaitAnswersItsCall is
  violated.`, with the trace `ArmWait(PENDING)` → `Supersede` → `CANCELLED` and `answers = 0`.
- **Code probe** (before the fix): `cargo test --offline --manifest-path core/Cargo.toml --lib
  wait_call_answer_gap_outside_the_drain_path -- --nocapture` printed `answers_for_wait_1=0` (satisfied at
  registration) and `answers_for_wait_2=0` (superseded).
- **Root cause**: only `wake_satisfied_at` (the drain path) appended a tool response for a wait's tool_call.
  The "satisfied at registration" branch of `import_response` and the cancellation branches of
  `submit_input`/`close_epoch_execution` moved the wait to SATISFIED/CANCELLED without answering. Strict wire
  endpoints (OpenAI-style Responses, Anthropic) reject an assistant `tool_calls` message without matching tool
  responses — a constraint stated in `core/src/kernel/mod.rs` and in the `wake_satisfied_at` comment, which is
  where the drain version came from.
- **Fix**: `core/src/v2/control.rs` gained `answer_closed_waits` (the answer lands on the wait's own call id
  and degrades to a note without one) and `wait_reason` (the drain and the registration path share one reason
  text), called from the registration branch of `import_response`, the supersede branch of `submit_input` and
  the sealing branch of `close_epoch_execution`. The deduplication key stays the wait id (`append_context`'s
  envelope deduplication), so a replay never appends a second answer.
- **Regression**: `core::v2::control::tests::wait_call_answered_outside_the_drain_path` (both paths assert the
  answer exists, is appended once and carries `superseded` for the supersede case);
  `the_wake_answers_the_wait_tool_call` (the drain path) still passes.

## 2. V-G1: a settled goal still took new work billed to it

- **Properties**: `NoStaleActiveGoal`, `RegisteredWorkNeedsAnActiveGoal`, `RequestsResolveToActiveGoals`
  (before the fix: the expected-counterexample configuration `MC_task_contract.cfg`).
- **Spec counterexamples** (re-runnable before the fix): `RegisteredWorkNeedsAnActiveGoal is violated`
  (a task delegated before its goal existed) and `ClosedGoalTakesNoNewOperation is violated` (a settled goal
  still opening a new operation).
- **Root cause**: `budget_goal` returned the instance's `active_goal_id` (or the goal of its oldest open task)
  without looking at the goal status; `delegate_task` only checked that `goal_id` existed; and
  `complete_goal`/`block_goal` left the instance pointer in place, so new turns kept billing a settled goal.
  The budget gate still worked (nothing over-reserved), but "settled" and "still receiving new work" were
  inconsistent.
- **Fix** in `core/src/v2/control.rs`:
  - `goal_is_active` plus `budget_goal`, which accepts only ACTIVE goals (both the instance pointer and the
    oldest open task path) and otherwise runs as "no goal", exactly like a session without one;
  - `detach_goal`, called when `complete_goal`/`block_goal` settle a goal, detaches every instance pointer to
    it and reports a `detached` count in the response and the events; a late finish on an already settled goal
    goes through the same idempotent detach;
  - `delegate_task` requires an ACTIVE goal and otherwise fails with a pointer to `create_goal` first.
- **Regression**: `core::v2::control::tests::a_settled_goal_takes_no_new_work` (pointer detached, delegation
  refused on both the explicit and the implicit identity path, new requests neither billed nor reserving,
  records frozen, and a fresh attached goal restoring both billing and delegation).
- **Deliberate semantic boundary**: the linearization point for new work is the **request**, not the
  operation — a request admitted while the goal was ACTIVE may still open operations and bill that goal after
  settlement (honest accounting). `complete_goal` checks open operations but not tasks, so a goal can settle
  while its own tasks are still open and those tasks' later requests have no billing goal. Tightening that
  would need a committed "completion refused" result for the driver and was out of scope; the boundary is
  recorded in the `verification/tla/V2Task.tla` header and in the verification README.

## 3. V-P1: a stale execution pointer after terminating an instance (found by the code-level invariants)

- **How it surfaced**: `core/tests/v2_invariants.rs` (the executable spec-to-code correspondence) reported
  `OneActiveRequest: instance i1 is MODEL_PENDING with 0 pending requests` during a random walk.
- **Root cause**: the TERMINATED branch of `set_lifecycle` cancelled the in-flight request through
  `close_epoch_execution` but, unlike `reset_instance`/`fail_request`, did not reset the execution pointer, so
  the instance stayed at `phase = MODEL_PENDING` with `active_request_id` pointing at a `CANCELLED` request.
- **Fix**: the termination branch in `core/src/v2/control.rs` applies the same normalization
  (`phase = 'READY'`, `active_request_id = NULL`).
- **Regression**: `core::v2::control::tests::terminating_an_instance_normalizes_its_execution_pointer`.
- **Added at the same time**: `core/tests/v2_invariants.rs` (every command sequence up to length 2 plus 60
  fixed-seed walks, re-checking 13 invariant groups after every step, with coverage assertions and a negative
  control for checker sensitivity).

## 4. V-P2: a compression request could be imported as a turn (found by the code-level invariants)

- **How it surfaced**: the random walk in `core/tests/v2_invariants.rs` (once coverage-driven) reached
  `record_attempt` → `import_response` applied to a **compression request** and succeeded.
- **Root cause**: `import_response` verified only that the request existed and was `PENDING`, never that
  `kind = 'turn'`, so a compression request could take the turn-import path (append an assistant entry, open
  operations, close out as a turn). A compression request must be submitted by `compress_context` as a
  summary (§7/A20); the driver never calls it that way, but the control plane did not refuse it.
- **Fix**: `import_response` in `core/src/v2/control.rs` now reads `kind` with the request and refuses
  anything that is not a `turn`, pointing at `compress_context` instead.
- **Regression**: `core::v2::control::tests::import_response_refuses_a_compression_request`.

## Incidental corrections

- `verification/tla/V2Wait.tla`: the supersede semantics now match the code ("cancel the batch and answer")
  and `ResolvedWaitIsAnswered` was added; `AnswerImpliesConditions` was split into
  `SatisfiedHoldsConditions` (only a wake carries the condition premise) and `AnswerImpliesResolved`.
- `verification/tla/V2Task.tla`: instance goal pointers and request admission (`Request`/`ImportOp`) are
  modeled, delegation gained an ACTIVE guard, and "terminal states are never rewritten" plus "no new work is
  registered on a settled goal" now use monitoring variables (`rewritten`/`lateTask`/`lateRequest`), because
  TLC only accepts action-carrying temporal formulas of the form `<>[]A`/`[]<>A`.
- The `make verify-model-contract` target and its two expected-counterexample configurations were deleted
  with the fix: both contracts are now invariants of the main configurations.
