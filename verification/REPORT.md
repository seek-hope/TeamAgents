# Formal verification report (2026-09-24)

This report is the **conclusion and ledger** of the formal-verification work: how far it proves things, with
what evidence, and what it does **not** prove. The property-by-property mapping to specs and code anchors is
in [README.md](README.md); the fix ledger is in
[review/fix-notes-verification-2026-09-24.md](../review/fix-notes-verification-2026-09-24.md).

## 1. Summary of conclusions

**What can be claimed**:

- The safety properties of nine protocol surfaces (control plane, artifacts/GC, waits/wakeups,
  tasks/delegation/goal settlement, compression, the daemon protocol, the required checks, the authority layer
  and the user's authority surface) hold under exhaustive TLC checking of the **abstract model**; liveness
  holds only under the explicitly stated weak fairness assumptions.
- The same invariants are recomputed against the **real `core::v2::Control`** by the executable
  correspondence test: every command sequence up to length 2 (38 commands, including refused combinations)
  plus 60 fixed-seed coverage-driven walks, re-checking 23 invariant groups after every step, with coverage
  assertions and a negative control for checker sensitivity.
- The kernel's pure functions (wire view, output capping, paging, response classification, argument hashing)
  are checked by bounded enumeration; the paging **arithmetic** additionally has a Kani machine proof, and the
  proven `page_span` is the **published function** (`page_output` calls it): two properties hold for **every
  `usize`** (no overflow, no overrun, equivalent `eof` test) and one (seamless page-by-page reconstruction)
  holds within the unwinding bound.
- The work found and fixed four code issues (V-W1/V-G1/V-P1/V-P2, each with a counterexample and a regression
  test) and corrected two properties that were written incorrectly (a vacuous property and a check that never
  actually ran).

**What cannot be claimed** (details in §5):

- **This is not a refinement proof**: the model is not the Rust implementation. Exhaustive results on the model
  do not automatically hold for the code, and the code-side results come from bounded exploration
  (enumeration plus sampling), not from "all executions".
- **Liveness only under assumptions**: the single liveness property (`V2Wait::NoStrandedPending`) depends on
  weak fairness of the parked drain, i.e. on the driver's poll loop continuing to run — an implementation fact,
  not a proven conclusion.
- **Bounded state spaces**: every enumeration runs on an explicitly bounded configuration (finite instances,
  tasks, requests and log lengths) and the frontier is recorded in the README (for example, `MC_task` diverges
  with two tasks).
- **Code outside the model**: provider adapters, MCP, Skills, TUI rendering and hit-testing, shell/bubblewrap
  isolation, process and job management, and real provider behaviour are all outside the formal scope. They
  are covered by sample tests and real-environment acceptance instead (see the "still uncovered" column in §4).

## 2. Evidence (all re-runnable)

| Layer | Evidence | Scale | Re-run |
|---|---|---|---|
| Protocol model | `tla/V2Control.tla` (15 invariants + 6 properties, incl. the deadline gate) | 132,193 states | `make verify-model` |
| Protocol model | `tla/V2Artifact.tla` (4 + 4) | 241 states | `make verify-model-all` |
| Protocol model | `tla/V2Wait.tla` (8 + 1 liveness) | 505,905 states | as above |
| Protocol model | `tla/V2Task.tla` (11) | 5,721,401 states | as above |
| Protocol model | `tla/V2Compress.tla` (8) | 8,467 states | as above |
| Protocol model | `tla/V2Daemon.tla` (10) | 51,713 states | as above |
| Protocol model | `tla/V2Checks.tla` (8) | 469 states | as above |
| Protocol model | `tla/V2Authority.tla` (11 invariants + 5 properties, three negative controls) | 270,288 states generated / 35,950 distinct | as above; controls via `make verify-model-counterexamples` |
| Protocol model (wide) | `MC_wide.cfg` (2 instances / 2 operations) | 275,004,673 states / 11 min 25 s (historical run; the files are unchanged: `git log -1 -- verification/tla/V2Control.tla MC_wide.cfg` = `d37e1b4`, the new hash after the 2026-09-25 history rewrite; two re-runs in this round reached about 170M / 250M states before the machine killed them, with no violation) | `make verify-model-wide` |
| Code-level correspondence | `core/tests/v2_invariants.rs` | 38 commands; 1,482 short sequences plus a 60×24-step walk | `cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants` |
| Pure functions | `core/tests/kernel_properties.rs` | 258 entry combinations plus a full paging enumeration | `cargo test --offline --manifest-path core/Cargo.toml --test kernel_properties` |
| Kani proofs | `kani/` (compiles the repository sources, 3 harnesses, 0 failures) | 2 properties for **every `usize`** plus 1 within a bound | `make verify-kani` (needs the Kani toolchain, ~7 s) |
| Gate | fmt + clippy `-D warnings` + all tests | 23 suites | `make check` |

Evidence that the checks are neither vacuous nor insensitive ("passing" is not because a check is too weak):

- Checker sensitivity: `the_invariant_checker_detects_broken_states` breaks state on purpose (unknown status
  values, a rewritten terminal state, a stale goal pointer, lifted coverage, coverage pointing at a
  non-summary) and the checker must report each one.
- Coverage assertions: the random walk must really reach "a wait resolved / a settled goal / a settled task /
  a terminal operation / an epoch reset / a terminated instance / a LIVE artifact / a submitted compression /
  a decided approval / a command replay / a dispatch refused after revocation".
- Non-vacuity on the model side: splitting the `V2Daemon` `checkpoint` into two steps made
  `SnapshotNeverLeadsCursor` fail immediately, and relaxing `V2Checks`' `Accept` (accepting as soon as one
  check passes) made `SuccessRequiresAllChecksPassed` fail immediately.
- The coverage assertion for wire-protocol pairing once caught a **no-op**: the first version located indexes
  by substring, so the pairing check never actually ran.

### Authority (added 2026-09-25, D-60)

`V2Grants.tla` + `MC_grants.cfg` model where a capability comes from: the session's bootstrap grants, narrowing
by an instance (manage covers message/delegate), the spawn-derived delegate grant, revocation with the parent
tree cascade and the revision bump, the dispatch re-check, and the rule that the offered tool surface only
contains what the grants back. Configuration: four bootstrap grants plus one free slot, two instances, the two
operations that matter (the leader's delegation, a spawned child's shell call).

| Run | Result |
|---|---|
| `make verify-model-all` (MC_grants) | **No error found** — 1,292,517 states generated / 178,024 distinct / 0 left / depth 11 / ~1 minute (all nine invariants plus the temporal `AuthorizedEffectsOnly`) |
| Falsification check (kept out of the tree) | Offering `shell` unconditionally — what the code did before D-60 — makes TLC report `Invariant OfferedToolsAreAuthorized is violated by the initial state`, so the property is sensitive to exactly that defect |
| Correspondence (`engine/tests/v2_supervisor.rs`) | `the_offered_surface_follows_the_grants` asserts the leader is offered `shell`/`spawn` and its spawned child is offered neither; it fails when the code stops filtering the surface by the grant |

### The goal's ceilings (added 2026-09-25, D-64)

The usage ceiling was already modelled (`BudgetFits`, `ReservationsAdmitted`, `AdmissionGate`, A18); the
*deadline* was not, and exposing both to the user (D-64) made that the gap to close. `V2Control`'s goal now
carries `deadlinePassed` (an environment action moves the clock past the deadline; the fact is monotone),
`BeginRequest` requires `~goal.deadlinePassed`, and `NoRequestAfterDeadline` states the gate.

| Run | Result |
|---|---|
| `make verify-model` (MC.cfg) | **No error found** — 6 s with the new invariant and property |
| `make verify-model-all` (MC_control_two.cfg) | **No error found** — 1,263,649 states / 165,792 distinct / ~45 s (two instances, the deadline flag included) |
| Negative control `MC_control_deadline.cfg` | a runtime that ignores the deadline (switch `IgnoreDeadline`) makes TLC report **`Action property NoRequestAfterDeadline is violated`**; `make verify-model-counterexamples` requires exactly that |
| Correspondence (`core/src/v2/control.rs`, `engine/tests/cli.rs`) | `goal_deadline_refuses_new_requests_and_dispatches` (the gate) and the daemon test that the configured `deadline_minutes` becomes an absolute deadline ~15 minutes out on the real goal, with the duration key never stored |

### The inbound boundary (added 2026-09-25, D-63)

`V2Control.tla` gained the inbound boundary: an instance field `queue` (set by `QueueInput` while a turn is in
flight, cleared by the boundary's `ApplyQueued` or by `Input`) and a monitor for the phase a user input landed
at. Properties: `InputLandsAtTheBoundary` (invariant — input never lands inside a turn whose request is
already fixed) and `QueuedInputEntersTheContext` (temporal — a queued input enters the context; a park keeps
it, a reset seals it with its epoch, termination ends it).

| Run | Result |
|---|---|
| `make verify-model` (MC.cfg) | **No error found** — 132,193 states generated / 27,488 distinct / ~6 s (15 invariants incl. `InputLandsAtTheBoundary`, six properties incl. `QueuedInputEntersTheContext` and `NoRequestAfterDeadline`) |
| Negative control `MC_control_midturninput.cfg` | the pre-D-63 behaviour (input applied inside the running turn) makes TLC report **`Invariant InputLandsAtTheBoundary is violated`**; `make verify-model-counterexamples` requires exactly that |
| `make verify-model-all` (MC_control_two.cfg, 2 instances) | **No error found** — 1,263,649 states generated / 165,792 distinct / ~45 s. This small two-instance configuration is what makes a *per-instance* liveness assumption testable: the wide configuration cannot finish in a reasonable time |
| Negative control `MC_control_two_disjunction.cfg` | the same two-instance configuration with the *older* fairness form (one disjunction over instances, as the model had before D-63) makes TLC report **`Temporal properties were violated`** — one instance stays dead while the other recovers, so its queued input never enters the context. Per-instance fairness (the code's one-driver-per-instance reality) removes it |
| `make verify-model-wide` (MC_wide.cfg) | not re-run to completion in this round: the extra instance fields and the two new temporal properties make it explore far more states than the historical 275M/11m25s run. It stays the broad, slow target; the small two-instance configuration above carries the fairness check |
| Correspondence (`core/src/v2/control.rs`, `engine/tests/v2_supervisor.rs`) | `an_input_inside_a_turn_waits_for_the_boundary` (READY applies; a turn in flight queues, reports `applied: false, queued: true` and leaves the phase alone; the drain applies it exactly once and last) and `an_input_arriving_during_a_turn_enters_at_the_next_boundary` (the real driver opens a second turn, and the input's index is greater than the first reply's) — the latter fails on the pre-fix code |

### The user's authority surface (added 2026-09-25, D-61)

`V2Authority.tla` + `MC_authority.cfg` model the surface a user drives the authority layer with: the view a
client reads (`id`, `issuer`, parent, revoked and the session revision), the pair table the surface refuses
against, a grant the user writes (optionally derived from one it already holds), revocation by a nameable id
with the subtree cascade, and the model-visible surface as a **cached** variable that only the instance's next
request refreshes. Configuration: three bootstrap grants plus one free slot (the user's grant, or the one a
spawn derives), two instances, one operation, two scopes.

| Run | Result |
|---|---|
| `make verify-model-all` (MC_authority) | **No error found** — 270,288 states generated / 35,950 distinct / 0 left / depth 12 / ~35 s (11 invariants plus five temporal properties) |
| Negative control `MC_authority_badview.cfg` | the view without the `id` field (the daemon before D-61) makes TLC report **`Temporal properties were violated`** — `EveryLiveGrantBecomesRevocable`: a user can list grants and still not name one |
| Negative control `MC_authority_trustsurface.cfg` | dispatch trusting the cached surface (the mistake §6.1/A04 forbids) makes TLC report **`Action property AuthorizedEffectsOnly is violated`** |
| Negative control `MC_authority_stalesurface.cfg` | a surface computed once and never recomputed makes TLC report **`Temporal properties were violated`** — `StaleSurfaceCatchesUp` |
| `make verify-model-counterexamples` | runs the three controls and **fails** if any of them verifies instead of being refuted, so none of the three claims can become vacuous unnoticed |
| Correspondence (`engine/tests/v2_supervisor.rs`) | `a_users_grant_reaches_the_workers_surface_at_the_next_request`: a spawned worker's first request has no `shell`, the user's grant (through the same `submit_user` path the daemon client uses) puts it on the next request, and revoking it takes the tool away again |
| Correspondence (`engine/tests/cli.rs`) | `the_authority_surface_grants_and_revokes_through_the_daemon`: the real binary against a real daemon — list carries the ids and the instances, the granted worker answers the dispatch question `holds_covering_grant(worker, "shell", "workspace")` with *true*, a derived grant dies with its parent, and after the revocation the question is *false* again |
| Falsification check (the defect this closed) | the standalone rusqlite probe recorded in D-61 shows the old view failing with `Invalid column type Real at index: 1, name: revoked_at` as soon as one grant was revoked |
| Real model (`review/dogfood/authority.py`) | DeepSeek Flash, native window, 2026-09-25: the worker reports it cannot run shell commands; the grant goes in (revision 8); the same worker then runs the command (exit 0, `proof.txt` present); the revocation goes in (revision 11) and no live shell grant is left — 13 model requests, no failed request |

**A property the model corrected.** The first formulation of the freshness claim was
`GrantReachesTheSurface == \A i : [](Entitled(i, "shell") => <>("shell" \in offered[i]))` — "once entitled,
always eventually offered". TLC refuted it, and the counterexample is a real behaviour: the user grants, then
*revokes before the instance takes its next turn*, so the correct surface at that next turn has no shell. The
property as stated would have demanded a tool the instance may no longer use. It is now `StaleSurfaceCatchesUp`
("a surface that lags the entitlement catches up at the next request"), which is what the code actually
guarantees, and the negative control above shows it still fails when the surface is never recomputed.

## 3. Issues found by verification (all fixed)

| ID | Issue | Spec counterexample | Fix and regression |
|---|---|---|---|
| **V-W1** | a wait's tool_call was answered only on the drain path; "satisfied at registration" and "superseded/closed epoch" left it unanswered, so a strict endpoint rejects the next request | `ResolvedWaitAnswersItsCall is violated` (`ArmWait → Supersede → CANCELLED, answers=0`) | `answer_closed_waits` extended to both paths; `wait_call_answered_outside_the_drain_path`; invariant `ResolvedWaitIsAnswered` |
| **V-G1** | a settled goal still accepted new work (delegation, opening operations, billing) | `RegisteredWorkNeedsAnActiveGoal`, `ClosedGoalTakesNoNewOperation` violated | `budget_goal` accepts only ACTIVE goals, settlement detaches the pointer, delegation requires ACTIVE; `a_settled_goal_takes_no_new_work`; invariants such as `NoStaleActiveGoal` |
| **V-P1** | a terminated instance kept a stale execution pointer (`phase = MODEL_PENDING` pointing at a cancelled request) | code-level invariant: `OneActiveRequest: instance i1 is MODEL_PENDING with 0 pending turn requests` | the termination branch normalizes like `reset_instance`/`fail_request`; `terminating_an_instance_normalizes_its_execution_pointer` |
| **V-P2** | `import_response` never checked `kind`, so a compression request could be imported as a turn | the walk reached the path and succeeded (the spec requires a refusal) | the control plane refuses `kind != 'turn'`; `import_response_refuses_a_compression_request` |
| Property fix | `V2Checks` first stated "SUCCEEDED implies the recorded verdict is pass" — a **vacuous** property (an action writes that variable itself) | relaxing `Accept` still "passed" | rewritten to bind the **observed check result**; relaxing it is then immediately refuted |
| Property fix | nested quantifiers in `V2Checks`' `BlockForInfra`/`BlockWhenExhausted` shared a name | parse error | separate quantifier variables |

## 4. Per-item ledger A01–A36

The "formal layer" column lists only what the model, the code-level correspondence or the pure-function layer
**really** covers; everything else relies on sample tests and real-environment acceptance (evidence in
`docs/ACCEPTANCE.md`).

| Item | Scenario | Formal coverage | Still uncovered |
|---|---|---|---|
| A01 | one Leader completes a goal | the `V2Control` phase machine, `OneActiveRequest`, `NoTurnWithoutWork`; code-level "one active turn request" | real provider turn behaviour |
| A02 | A→B→C→A communication | code-level: envelope deduplication, grants as prerequisites, goal pointer | topology/cycles themselves |
| A03 | limited delegation and parent revocation | code-level: a dispatch after revocation is refused (the `!dispatch_without_grant` probe plus a coverage assertion); model `StaleExecutorRejected` | cascading semantics of the grant tree |
| A04 | a queued action meets a revocation | as above (dispatch after revocation must be refused) | queuing/re-authorization timing |
| A05 | reading another instance's history | — | sample tests |
| A06 | a message applied across a restart | `V2Control` crash/recovery and envelope deduplication; code-level receipt stability and replay inertia | — |
| A07 | permanent start failure | `V2Control::FailRequest` (releasing the reservation) | parking semantics, real spawn failures |
| A08 | crash after a tool succeeded, before consumption | `V2Control`: `RecordBeforeEffect`, `EffectAtMostOnce` | — |
| A09 | unknown external outcome | `V2Control`: an unknown outcome is never replayed | the code-level task-parking path |
| A10 | duplicate dispatch / GO | `V2Control`: effect at most once | the job layer |
| A11 | daemon and runner crash separately | the `V2Control` recovery path | process/job layer |
| A12 | a service outlives an exit | — | real process evidence (D-41) |
| A13 | cancel / timeout / completion races | `V2Control`: `CancelledBeforeStartHasNoEffect`, `TerminalOpStable` | — |
| A14 | bubblewrap unavailable | — | real isolation-environment evidence |
| A15 | environment identity | — | process-identity evidence |
| A16 | a required check fails | all 8 `V2Checks` properties (including "only self-reported successes are verified") | the execution details of `execute_check_ops` |
| A17 | artifacts change after a check | the stale cases in `V2Checks` and `BlockedAfterTheBudgetOrStale` | real file-hash re-verification |
| A18 | multi-instance usage budget | `V2Control` (reservation ceiling, admission gate, release) plus `V2Task` (the `budget_goal` resolution rules) | real provider billing |
| A19 | truncated stream and connection loss | `V2Control`: `SelectionIsComplete`; code-level "a selected attempt is complete" | provider retry details |
| A20 | restart after compaction | all 8 `V2Compress` properties plus 5 code-level groups (monotone coverage, originals never lost, tail append) | — |
| A21 | the user adjusts an instance directly | — | sample tests for the single-writer context |
| A22 | wait cycles and timers | `V2Wait` (including "a parked PENDING is eventually closed or superseded") | the graph algorithm of cycle detection itself |
| A23 | a result arrives before the wait is registered | `V2Wait` evaluation at registration plus code-level `ResolvedWaitIsAnswered` | — |
| A24 | a late result after a reset | `V2Control::NoReceiptAcrossEpochs` | — |
| A25 | MCP approval / cancellation / unknown outcome | model `NoEffectBeforeApproval`; code-level approval finality, pending approvals only on PREPARED operations, no effect after a denial | MCP transport and tool surface |
| A26 | Skills permissions | — | real symlink/registration-root evidence |
| A27 | heterogeneous providers cooperating | — | real two-sided message evidence |
| A28 | disconnect, slow client, reconnect | all 10 `V2Daemon` properties plus code-level receipt stability, replay inertia and append-only logs | the real socket layer (covered by the daemon tests) |
| A29 | session isolation and a shared project | the single-session constraint in `V2Control` | shared-directory authorization |
| A30 | artifact and DB write boundaries | all 8 `V2Artifact` properties plus code-level "LIVE has bytes" | real power loss |
| A31 | write failure / disk full | — | `StorageFull` classification and real SQLite FULL injection |
| A32 | very large history measurement | pure-function seamless paging reconstruction (the coordinate semantics of readback) | the performance numbers themselves |
| A33 | two daemons / stale lock | — | real lock and second-daemon evidence |
| A34 | incompatible schema | — | stamp/migration sample tests |
| A35 | goal deadline | the deadline gates in `V2Control` (`AdmissionGate` plus a hard dispatch refusal) | real clock boundaries |
| A36 | install / init / doctor / cleanup | — | CLI and cleanup evidence |

Subtotal: **25 items** have a non-empty "formal coverage" entry (A01–A04, A06–A11, A13, A16–A20, A22–A25,
A28–A30, A32, A35) and the remaining **11** (A05, A12, A14, A15, A21, A26, A27, A31, A33, A34, A36) have
**only** sample tests and real-environment evidence today. **No item claims to be finished by formal means
alone**, and conversely formal coverage does not excuse an item from sample or real-environment acceptance.

## 5. Unproven list (honest boundaries)

1. **The model is not the code.** The model is an abstract state machine; the code-level correspondence is
   bounded exploration, not a refinement proof. Crossing that line would require mapping every invariant onto
   an executable assertion in the code (done) **and** proving that every implementation step lies within the
   model's step set (not done).
2. **Liveness**: `V2Wait::NoStrandedPending` depends on weak fairness of the parked drain; `V2Control`'s
   liveness depends on three assumptions about the driver and the provider (D-63): weak fairness of each
   instance's recovery (`\A i : WF(Recover(i))`), strong fairness of each instance's boundary drain
   (`\A i : SF(ApplyQueued(i))`, strong so that a crash loop cannot starve it) and weak fairness of each
   instance's turn taking its next step (`\A i : WF(TurnStep(i))` — the provider eventually answers or the
   transport times out into `FailRequest`). All three are assumptions about a scheduler and a provider outside
   the verified system. The fairness is deliberately *per instance*: the older
   `WF_vars(\E i : Recover(i))` let one instance's recovery carry the other's starvation, which the
   two-instance configuration refutes once a per-instance liveness property exists.
3. **Boundedness**: every enumeration stays inside a bounded configuration (finite requests, attempts, tasks
   and log lengths, `MaxOps` and so on). Unbounded counters (budget, usage, event sequence) appear in the
   model only as bounded placeholders.
4. **Code outside the model**: provider adapters and retry classification, MCP, Skills, TUI rendering and
   hit-testing, shell and bubblewrap isolation, job/process lifetimes and real provider behaviour. These are
   covered by sample tests and real-environment acceptance only.
5. **Concurrency**: `Control::submit` is serialized on a single connection (a single writer) and the model
   does not cover interleavings across connections; the daemon's concurrent read and write connections appear
   only in A28's structural statement that a slow client cannot block the writer, without an exhaustive
   interleaving.
6. **Pure-function layer**: the paging arithmetic has a Kani machine proof and the proven `page_span` is the
   published function (`page_output` calls it), with two properties holding for **every `usize`**. What is not
   covered: (a) `page_output`'s argument parsing goes through serde_json, where symbolic coordinates degrade
   numeric comparisons into a symbolic `memcmp` (measured not to converge), so argument validity is only
   enumerated over concrete values; (b) `cap_tool_output`'s 24000-character threshold cannot be expanded and
   likewise only has concrete tests at boundary lengths; (c) the remaining pure functions
   (`prepare_request`'s view, `interpret_response`'s classification, `args_hash`) are only reached by bounded
   enumeration. Lean 4 was not adopted: it is an interactive theorem prover that needs the elan toolchain and
   hand-written proof scripts, and this round prioritised one more protocol surface, the code-level
   correspondence and applying Kani where it converges on the published function; the upgrade path is in the
   README's later phases.

## 6. Re-running and what would invalidate this

```bash
make verify-model-all     # exhaustive configurations for the seven protocol surfaces (seconds to ~20 s)
make verify-model-wide    # wide control-plane configuration (~11 minutes / 275M states)
make verify-model-all     # all eight small modules (~80 s, of which the authority model is ~60 s)
make check                # fmt + clippy -D warnings + 23 suites (including the two code-level layers)
make verify-kani          # Kani proofs for the paging arithmetic (needs the Kani toolchain; ~1 s)
```

Any one of the following invalidates the conclusions above and requires a re-run and an update:

- a spec file changes (a property weakened, an action relaxed) — TLC only proves the spec as it was;
- a code-level coverage assertion fails (the walk no longer reaches some key state);
- the checker-sensitivity test fails (the checker can no longer find deliberate breakage);
- the gate or `verify-model-*` reports a violation;
- a new deviation is found (for example another state the model considers impossible but the code reaches).
