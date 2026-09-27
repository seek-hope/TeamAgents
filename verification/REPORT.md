# Formal verification report (2026-09-24)

This report is the **conclusion and ledger** of the formal-verification work: how far it proves things, with
what evidence, and what it does **not** prove. The property-by-property mapping to specs and code anchors is
in [README.md](README.md); the fix ledger is in
[review/fix-notes-verification-2026-09-24.md](../review/fix-notes-verification-2026-09-24.md).

## 0. Gate status (re-run 2026-09-27 at `6363b312`)

* `make verify-model-all` was re-run on this tree: all **19** configurations report `No error has been found`,
  in 7 m 21 s (the newest four are the retention rule, D-192, the task model's second task, D-218, the
  approval window, D-225 — 14,225 states / 3,136 distinct — and the config trust gate, D-244, which is
  exhaustive in 9 s: 353,217 states generated / 25,376 distinct). The largest is **`MC_task.cfg`** with 5,721,401 states generated / 606,904 distinct (its one-instance two-task sibling `MC_task_two.cfg` 612,802 / 56,074); `MC.cfg`
  itself generates 84,877 / 18,384, and the smallest, `MC_store.cfg`, 48 / 13; the job handshake's
  `MC_jobs.cfg` generates 207 / 64 the inbox's `MC_inbox.cfg` 793 / 211 the write-failure latch's
  `MC_diskfull.cfg` 63 / 22 and the coordinator lock's `MC_coordinator.cfg` 51 / 16. Every count that predates this
  entry is identical to the previous run on the same tree — what a deterministic checker on unchanged inputs should print, and the
  reason these numbers describe the *material*, not a machine. (The line once called `MC.cfg` the largest and
  quoted `MC_task`'s numbers for it — a mis-attribution no gate looked at, found by re-running the target and
  reading its output per configuration, D-159.)
* `make verify-model-counterexamples` was re-run: all **53** negative controls are refuted, each naming its
  property (`AuthorizedEffectsOnly`, `InputLandsAtTheBoundary`, `NoRequestAfterDeadline`, `NoTurnWithoutWork`
  twice, `SettlementFollowsATurnAfterTheLanding`, `NoForeignAdoption`, three temporal refutations, the
  four retention guards: `NoReferenceToEvictedFact`, `EvidenceIsNeverEvicted` and `OnlyOldFactsAreEvicted`
  twice, the four task controls D-218 added: `RegisteredWorkNeedsAnActiveGoal`, `RequestsResolveToActiveGoals`,
  `DependenciesPointBackwards` and `NoOpenTaskOnDeadAssignee`, the seven D-219 added: `NoEntryIsEverLost`,
  `CoverageNeverLifted`, `RequestClosesOnce`, `ReceiptsAreStable`, `LogMonotone`, `NoResyncInThisVersion` and
  `RoundsAreMonotone`, the three D-220 added: `GcClaimsOnlyUnreferencedLive`, `ResolvedWaitIsAnswered` and
  `OfferedToolsAreAuthorized`, and the four D-225 added: `NoLateEffect`, `ApprovalDecisionIsFinal`,
  `TerminalOperationHasNoPendingApproval` and `ParkedHasAnApprovalRow`, and the five D-244 added:
  `NothingFromTheProjectUntrusted`, `UserDefinitionsNeverOverridden`, `PolicyClassesStayTheUsers`,
  `TrustOnlyFromTheUser` and `RefusalsAreNamed`), in 5 m 25 s.
* The wall clocks above are upper bounds, not machine-independent figures: they were measured while this
  machine carried a load average of about 140 on 20 cores (the standing host-cleanup item), and TLC is
  CPU-bound. The probe in A32 now records the same conditions next to its numbers for the same reason (D-188).
* **The Kani layer was re-run on this tree** (2026-09-27; first re-run 2026-09-26, D-132): `make verify-kani`
  reports `Complete - 3 successfully verified harnesses, 0 failures, 3 total` in ~3 s with a cached build
  (the 2026-09-26 run paid for the build: ~16 s). The toolchain this report called missing is
  installed after all, at the path the Makefile's `KANI_PATH` already points at: `~/.cargo/bin/kani` reports
  Kani 0.68.0 with CBMC 6.11.0, and `verification/kani/target/` had last been written on 2026-09-25. `make
  verify-kani` reports `Complete - 3 successfully verified harnesses, 0 failures, 3 total`, so the paging
  arithmetic is verified on the current sources, not carried over by identity. The target was also shown able
  to fail: with `kernel_types::page_span` mutated to `limit.min(total.saturating_sub(offset)).max(1)` the same
  command reports `1 successfully verified harnesses, 2 failures` and exits 1; the mutation was reverted
  byte-identically (`git diff` empty) before the green re-run. The identity argument still stands as history
  (`git log -L :page_span:core/src/kernel/types.rs` shows one commit, `0d4c057`, and the harness crate changed
  only in comments since, `c322677`) — it is just no longer what the result rests on. `make verify-kani` also
  fails loudly instead of passing on a `grep` that matched "failed" (D-122).
* **What those counts cover is now checked** (2026-09-27, D-185): `review/verification_catalogue.py` runs inside
  `make hygiene` and holds `verification/tla/` against the `verify-model*` targets that drive it and against
  the two counts stated above — every configuration and module on disk must be driven and described in
  `verification/README.md`, and a configuration dropped from a list is a finding. Before it, a `.cfg` file
  nothing ran and a count that had drifted both passed every gate.
* **A claim has to be able to fail** (2026-09-27, D-219): the same audit fails a variable no action ever
  changes. A variable the model cannot move is a constant, so every claim over it holds for want of a step —
  six were carrying claims (`lost`, `uncovered`, `pruned`, `drift`, `shrank`, `rewound`), and `V2Compress`'
  `RequestClosesOnce` was entailed by `TypeOK`. All seven now have a counterfactual step and a control that
  refutes them; §3 carries the entry.
* **The heading's commit is held against the material** (2026-09-27, D-202): §0 opened with a pin — `f521fd4f`,
  the commit D-184 was written at — that could no longer have produced the numbers below it, because it predates
  `verification/tla/MC_retention.cfg` while the same section counts "the twelfth is the retention rule, D-192".
  `review/verification_catalogue.py` now requires the pin to be a commit in this repository at or after the
  newest change to the material a re-run covers (`verification/tla`, `verification/kani`, and the sources the
  harness crate compiles in with `#[path]`). The same class was found one document over, in
  `docs/ACCEPTANCE.md`'s A32, whose pin preceded the commit that added the conditions it reports; that pin is
  repaired, and `review/requirement_trace.py` holds a row's pin against the example its command names.

* **The job handshake is modelled now** (2026-09-27, D-206): `V2Jobs.tla` is the twelfth module — the journal's
  phases, the acceptance persisted before the spawn, a duplicate GO as a no-op, CANCEL before the start as final,
  and the recovery read as one atomic snapshot of journal and effect ("did not run" only for a READY journal, the
  unverifiable band is `unknown`), with `SettledRunnerLeaves` for D-153's rule that a settled job's runner goes
  away. Its four negative controls each forget one rule and are refuted. Writing it produced the defect the
  recipes could not see — an `UNCHANGED` list that made a counterfactual inert, so a control *verified* instead of
  refuting, with TLC reporting it only as a warning nothing grepped for — and the two targets now treat a
  `Warning:` as a failure.

* **The inbox is modelled now** (2026-09-27, D-207): `V2Inbox.tla` is the thirteenth module — the envelope
  persisted as accepted, the exactly-once application whose dedup key is the envelope id (so a replay after a lost
  marker appends nothing), the sequence order, the stale-epoch seal, the bound that gives a full inbox its
  backpressure, and the drain's identity check — with five negative controls that each forget one rule. It is
  safety only, with no fairness assumption (whether a drain runs is the client's business), and `MC_inbox.cfg` is
  793 states / 211 distinct. The model exposed one of its own traps while being written: `x' = x \/ cond` is a
  *disjunction* in TLA+, not an assignment, and TLC says so statically ("successor state is not completely
  specified") — the second time this campaign's modelling work found an audit-side defect rather than a
  product-side one.

* **The write-failure latch is modelled now** (2026-09-27, D-208): `V2DiskFull.tla` is the fourteenth module —
  the latch a failed submit sets, the step that never runs again, the park retried at poll pace that names the
  in-flight persistence loss, and the clear that needs the park to land — with four negative controls. Its
  liveness half had to be stated honestly twice: weak fairness is not enough (the environment can free space only
  intermittently), and even under strong fairness a *single* writable moment proves nothing, because the disk can
  refill before the next poll — the property is `[](latched /\ []<>(store = "ok") => <>(~latched))`, the retry
  live *while writability recurs*. `MC_diskfull.cfg` is 63 states / 22 distinct, and no run printed a `Warning:`.

* **The coordinator lock is modelled now** (2026-09-27, D-210): `V2Coordinator.tla` is the fifteenth module — one
  coordinator per state root, a restart that lands in the *fork window* (a tool child inherits the descriptor
  table; CLOEXEC closes it at `exec`) waiting instead of reporting a coordinator that is not really there, and the
  other half of the rule: with no coordinator alive the only thing that may still hold the lock is a child in that
  window, so nothing blocks recovery. Three negative controls forget one rule each and are refuted;
  `MC_coordinator.cfg` is 51 states / 16 distinct, and no run printed a `Warning:`.

* **The checked set is held against the modules' own claims** (2026-09-27, D-212): every configuration a
  `verify-model*` recipe runs must be mapped to its module explicitly (the small configurations rode a `case`
  default that would silently run the wrong one, and `MC_artifact.cfg` was relying on it), and every name a module
  marks between its `invariants --`/`properties --` markers must be listed by a configuration that runs it. The
  rule found `V2Compress`' `RequestClosesOnce` defined and listed nowhere — a claim nothing checked, which holds
  (8,467 states / 796 distinct, measured) but which `docs/ACCEPTANCE.md`'s A20 row *already counted as covered*
  ("all eight `V2Compress` properties"): the row was an over-claim until this decision made it true. It also found
  `V2Grants`' four `TypeOK*` copies (one describing a field name the state no longer had) and `V2Store`'s
  `RefusalIsSilent` alias, and closed the Makefile's implicit default.

## 1. Summary of conclusions

**What can be claimed**:

- The safety properties of seventeen surfaces (control plane, artifacts/GC, waits/wakeups,
  tasks/delegation/goal settlement, compression, the daemon protocol, the required checks, the authority
  layer, the user's authority surface, session-store identity, and — added 2026-09-27, D-225 — the approval
  window, and — added 2026-09-27, D-192 — the
  retention rule, which D-192 modelled before the destructive code and D-245 implemented against it — and,
  added 2026-09-27, D-206, the job handshake with its recovery verdict, and — added 2026-09-27, D-207 — the
  inbox: the exactly-once application, the sequence order, the bound, the stale-epoch seal and the drain's
  identity check, the write-failure latch with its park — added 2026-09-27, D-208 — and, added 2026-09-27,
  D-210, the coordinator lock with its fork window, and, added 2026-09-27, D-244, the repository-local
  config's trust gate: what a cloned repository may contribute, never silently dropped and never able to
  grant itself the opt-in)
  hold under exhaustive TLC checking of the **abstract model**; liveness holds only under the explicitly
  stated weak fairness assumptions.
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
  tasks, requests and log lengths) and the frontier is recorded in the README. The task product is the concrete
  example (D-218): 2 tasks with one instance and one goal is exhaustive (612,802 states), while 2 tasks with 2
  instances and 2 goals did not converge in five minutes (33.6M generated / 7.9M distinct, queue still growing),
  and the symmetry the README named as the upgrade cut that by only about 1.6× — so it stays a *search* target
  (`make verify-model-sim SIM_CONFIG=MC_task.cfg`), not a proof.
- **Code outside the model**: provider adapters, MCP, Skills, TUI rendering and hit-testing, shell/bubblewrap
  isolation, process and job management, and real provider behaviour are all outside the formal scope. They
  are covered by sample tests and real-environment acceptance instead (see the "still uncovered" column in §4).

## 2. Evidence (all re-runnable)

| Layer | Evidence | Scale | Re-run |
|---|---|---|---|
| Protocol model | `tla/V2Control.tla` (16 invariants + 6 properties, incl. the deadline gate, the committed-tail rule and the landing-attribution rule) | 84,877 states | `make verify-model` |
| Protocol model | `tla/V2Artifact.tla` (4 + 4, one counterfactual constant since D-220) | 241 states | `make verify-model-all`; the control via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Wait.tla` (8 + 1 liveness, one counterfactual constant since D-220) | 505,905 states | as above; the control via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Approval.tla` (6 claims, four counterfactual constants since D-225) | 14,225 states / 3,136 distinct | `make verify-model-all`; the four controls via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Task.tla` (11, four counterfactual constants since D-218) | `MC_task.cfg` 5,721,401 states; `MC_task_two.cfg` 612,802 states / 56,074 distinct | as above; the four controls via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Compress.tla` (8, three counterfactual constants since D-219) | 8,467 states | as above; the three controls via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Daemon.tla` (10, three counterfactual constants since D-219) | 51,713 states | as above; the three controls via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Checks.tla` (8, one counterfactual constant since D-219) | 469 states | as above; the control via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Trust.tla` (6 claims, five counterfactual constants since D-244) | 353,217 states generated / 25,376 distinct (9 s) | as above; the five controls via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Authority.tla` (11 invariants + 5 properties, three negative controls) | 270,288 states generated / 35,950 distinct | as above; controls via `make verify-model-counterexamples` |
| Protocol model | `tla/V2Store.tla` (6 invariants + 3 properties, one negative control) | 48 states generated / 13 distinct | as above; control via `make verify-model-counterexamples` |
| Protocol model (wide) | `MC_wide.cfg` (2 instances / 2 operations) | 275,004,673 states / 11 min 25 s (historical run of `d37e1b4`, before the 2026-09-25 history rewrite). The spec has changed since (D-63/D-64/D-65/D-71 add instance fields, the deadline flag and the committed-tail rule), so that number is history: re-runs in this round reached about 170M / 250M states, and the D-71 re-run reached **36.5M states generated / 7.6M distinct / 29 min, 4.6M still queued, no violation** before it was stopped under the turn's time bound. The wide configuration stays the slow, best-effort target; the small two-instance configurations carry the per-instance checks in `make verify-model-all` | `make verify-model-wide` |
| Code-level correspondence | `core/tests/v2_invariants.rs` | 38 commands; 1,482 short sequences plus a 60×24-step walk | `cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants` |
| Pure functions | `core/tests/kernel_properties.rs` | 258 entry combinations plus a full paging enumeration | `cargo test --offline --manifest-path core/Cargo.toml --test kernel_properties` |
| Kani proofs | `kani/` (compiles the repository sources, 3 harnesses, 0 failures) | 2 properties for **every `usize`** plus 1 within a bound | `make verify-kani` (re-run 2026-09-27: 3 harnesses, 0 failures, ~3 s with a cached build, ~16 s cold; the negative control flips it to exit 1 — D-132) |
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

### The turn storm (added 2026-09-25, D-65)

The probe's runaway turned out to be a deviation from an **already-verified property**: `V2Control`'s
`NoTurnWithoutWork` forbids `MODEL_PENDING`/`TOOLS_PENDING` while the last word is the model's own, and the
code's idle rule (""no open tasks"") re-opened a turn in exactly that state whenever an instance owed a task.
The model needed almost nothing new — the counterfactual switch `ReaskAfterReply` — because the property was
already there and simply never enforced against this path.

| Run | Result |
|---|---|
| `make verify-model` (MC.cfg) | **No error found** — 6 s, unchanged |
| Negative control `MC_control_reask.cfg` | re-opening a turn while the last word is the model's own (the pre-D-65 code) makes TLC report **`Invariant NoTurnWithoutWork is violated`**; `make verify-model-counterexamples` requires exactly that |
| Correspondence (`engine/tests/v2_supervisor.rs`) | `a_prose_reply_leaves_one_turn_and_the_delegator_resolves_the_task`: one request, the task `RUNNING`, the leader `WAITING`, the user cancels, the leader wakes, the goal settles. With the pre-fix clause restored the same test fails (`4` requests in 1.5 s instead of `1`) |
| Real model (measured before the fix) | 169 requests / 1,226,717 prompt tokens / 181 context entries in ~15 minutes answering `BLOCKED.` as prose, no progress (`review/dogfood/authority.py`, first prompt shape; ACCEPTANCE's known gaps held the numbers and now points at A07) |

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

### A run's outcome follows its own input (added 2026-09-25, D-72)

`exec` may be asked while the leader is already in a turn; its input is then queued and lands at the next
boundary (D-63). D-72 asks what that run's *outcome* may be, and the answer is a property rather than a
convention: an outcome recorded before the input landed belongs to a turn the input was not part of, so the
client reads its own entry and the first turn-ending entry after it. The premise is exact — a landing puts the
input in the context the next request is fixed from — so D-72 states it as `SettlementFollowsATurnAfterTheLanding`
(a settlement must follow a request begun since the instance's last landing) and records it in a monitor when
the goal settles. `SettleGoal` also gained the abstraction it was missing: it now requires `tail = "assistant"`,
i.e. the settlement is about the model's own completion, which is what both code paths do.

| Run | Result |
|---|---|
| `make verify-model-all` (MC.cfg) | **No error has been found** — 84,877 states generated / 18,384 distinct / 0 left (the guard narrows the graph: a settlement now needs the model's own response) |
| `make verify-model-all` (MC_control_two.cfg) | **No error has been found** — 958,777 states / 131,440 distinct / 0 left |
| Negative control `MC_control_landing.cfg` | storing the input *inside* the running turn (the pre-D-63 switch `AllowMidTurnInput`) makes TLC report **`Invariant SettlementFollowsATurnAfterTheLanding is violated`**, and the counterexample is the story: `BeginRequest → MidTurnInput → RecordAttempt → ImportResponse → SettleGoal` with `settledAfterTurn = FALSE` — the settlement of a turn that never saw the input |
| Correspondence (`engine/tests/v2_daemon.rs`) | `a_queued_input_is_not_answered_by_the_previous_turns_settlement` (real socket: the queued run reports `end: reply` + its own answer + `goal_status: null` while the session's goal is `SUCCEEDED`; **fails with the pre-fix decision restored**: `left: Completed, right: Reply`), `a_queued_input_is_not_answered_by_the_previous_turns_reply`, `a_queued_input_a_reset_sealed_is_reported_undelivered` |
| Pure rule (`engine/src/v2/exec.rs`) | `v2::exec::tests::an_outcome_before_the_runs_own_input_is_not_its_outcome` enumerates the shapes: an input that never landed, an earlier settlement, tool traffic inside the run's own turn, a later turn's entries, the run's own settlement, a closed turn, and a bare tool call |
| Real models (`review/dogfood/queued_input.py`) | deepseek: run 1 `completed`/`SUCCEEDED`, the queued run `reply`/`BANANA`/`goal_status: null` (2.8 s) — kimi: the same shape (40.4 s). Against the pre-fix build the harness fails on the queued run with `end=completed / goal=SUCCEEDED / reply=null` |

### The runtime's own word (added 2026-09-25, D-71)

`NoTurnWithoutWork` (D-65) said "no turn while the last word is the model's own", and the code's idle rule
matched that clause literally: a turn opened unless the last context entry was the model's (`assistant`). The
runtime's own closing notes were stored *as* assistant entries to keep an instance idle, so the model had one
tail value for two speakers — and the headless client, reading "the last assistant entry is the reply",
reported the runtime's block note as the member's answer with exit 0 (a real Kimi run; D-71). The model now
distinguishes the two speakers: the tail is committed when it is the model's own text (`"assistant"`) **or**
the runtime's closing note (`"runtime"`, which `SettleGoal` now leaves), and a turn opens only for the
unaddressed tails (`UnaddressedTails == {"user", "tool"}`). `CommittedTails` is the shared statement, so the
invariant and the guard cannot drift apart.

| Run | Result |
|---|---|
| `make verify-model-all` (MC.cfg) | **No error found** — 137,401 states generated / 28,688 distinct / 0 left / depth 15 (the guard change makes the state graph slightly smaller; `NoTurnWithoutWork` reads `tail \notin CommittedTails`) |
| `make verify-model-all` (MC_control_two.cfg) | **No error found** — 1,316,137 states generated / 173,184 distinct / 0 left |
| Negative control `MC_control_runtimeTail.cfg` | treating the runtime's own closing note as unaddressed work (switch `RuntimeTailIsWork`, exactly the clause the old rule implied) makes TLC report **`Invariant NoTurnWithoutWork is violated`**, with the violating state showing `tail = "runtime"` and `phase = "MODEL_PENDING"` — i.e. the runtime re-opening a turn against its own settlement. `make verify-model-counterexamples` requires exactly that, so the new clause is not vacuous |
| Correspondence (`engine/tests/v2_daemon.rs`) | `a_runtime_blocked_goal_is_not_reported_as_a_reply`: a real daemon, a socket, three repair rounds against a check that never passes — the run reports `failed` / `BLOCKED` / `reply: null` / exit 1, the settlement is an event (`blocked_by: runtime`), and the instance's tail entry is of kind `runtime` with the user's role |
| Correspondence (`engine/tests/v2_daemon.rs`) | `a_turn_closed_by_the_runtime_without_a_settlement_is_not_a_reply`: the second run on an already-settled goal reports `unsettled` (exit 1) in under a second of work instead of reading the closing note as a reply or waiting out the deadline |
| Correspondence (`core`, `tui`) | `closing_a_turn_answers_its_finish_call` (the marker is a `runtime` entry in the user's voice; the finish call is still answered next to it), `migrate_rewrites_the_runtimes_closing_notes` (a schema-2 store is rewritten: the runtime's notes only, the member's answer and the tool receipt untouched), `the_runtimes_closing_note_is_not_the_members_message` (the TUI renders it as the runtime's, never as the instance's) |
| Real model, both protocols (`review/dogfood/checks.py`) | deepseek: exit 1, `end=failed`, goal `BLOCKED`, 11 requests, 9.7 s — kimi (`responses`): exit 1, `end=failed`, goal `BLOCKED`, 8 requests, 37.8 s. The Kimi run reported `exit 0 / end=reply / goal=None` before the fix |

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
| `make verify-model-wide` (MC_wide.cfg) | **re-attempted 2026-09-27 and still not to completion**: the attempt was bounded to one hour and terminated by that bound (exit 124) after writing 2.9 GB of TLC state store, against the historical 275M states / 11 m 25 s — the instance fields and temporal properties added since then put a complete run beyond a bounded attempt. It stays the broad, slow target; the small two-instance configuration above carries the fairness check |
| `make verify-model-sim` (MC_wide.cfg, the default) | **no invariant violated** in 20,000 random behaviors of depth 100 — 2,022,792 states checked in ~4 minutes (seed 11, one worker, 2026-09-27). The supplement D-210's ceiling named: it *searches* the space the exhaustive run cannot reach, so a violation found here would be real while absence proves nothing, and it checks no temporal property (the small configurations do that exhaustively). Controls: the same target on a refuted configuration (`SIM_CONFIG=MC_control_midturninput.cfg`) fails with the violation named, and on a configuration the mapping block does not name it reports "did not run" instead of a violation |
| `make verify-model-sim SIM_CONFIG=MC_task.cfg` | **no invariant violated** in 20,000 random behaviors of depth 100 — 7,663,011 states in 2 m 13 s (2026-09-27): the task configuration's second-task space diverges under the exhaustive target, so this searches it instead. The same weight and the same ceiling as the row above |
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

### Session-store identity (added 2026-09-26, D-87)

`V2Store.tla` + `MC_store.cfg` model what a session finds when it opens a state root: an empty path, a path
already holding another program's database (with and without a format id of its own), our own file with a
valid stamp, our own file with the schema written and the stamp missing (the crash between the two writes),
and the read-only open. The environment can write a foreign database into an empty path, so the guard is
exercised against a real foreign file rather than an assumption, and the counterfactual `IgnoreForeign` is the
D-87 defect itself.

| Run | Result |
|---|---|
| `make verify-model-all` (MC_store) | **No error found** — 48 states generated / 13 distinct / depth 3 (6 invariants plus three temporal properties, one of them the leads-to claim that a half-initialized database is completed rather than stranded) |
| Negative control `MC_store_adopt.cfg` | `create = true` initializing whenever there is no stamp (the store before D-87) makes TLC report **`Invariant NoForeignAdoption is violated`** |
| Correspondence (`core/src/v2/store.rs`) | `open_never_adopts_an_unstamped_file_that_holds_foreign_tables` (the refusal names the foreign tables, and the file is compared byte for byte with what it held before) and `open_completes_a_session_database_that_lost_its_stamp_to_a_crash` |
| Correspondence (`review/dogfood/boundary.py`) | the real binary on such a root: `doctor` exits 1, `exec` refuses in 0.2 s with exit 2 naming the file and the foreign table, and the file's SHA-256 is unchanged |

## 3. Issues found by verification (all fixed)

| ID | Issue | Spec counterexample | Fix and regression |
|---|---|---|---|
| **V-W1** | a wait's tool_call was answered only on the drain path; "satisfied at registration" and "superseded/closed epoch" left it unanswered, so a strict endpoint rejects the next request | `ResolvedWaitAnswersItsCall is violated` (`ArmWait → Supersede → CANCELLED, answers=0`) | `answer_closed_waits` extended to both paths; `wait_call_answered_outside_the_drain_path`; invariant `ResolvedWaitIsAnswered` |
| **V-G1** | a settled goal still accepted new work (delegation, opening operations, billing) | `RegisteredWorkNeedsAnActiveGoal`, `ClosedGoalTakesNoNewOperation` violated (the first is refutable again since D-218: `MC_task_delegates_to_settled.cfg`) | `budget_goal` accepts only ACTIVE goals, settlement detaches the pointer, delegation requires ACTIVE; `a_settled_goal_takes_no_new_work`; invariants such as `NoStaleActiveGoal`, and since D-218 the billing half too (`MC_task_bills_settled.cfg`) |
| **V-P1** | a terminated instance kept a stale execution pointer (`phase = MODEL_PENDING` pointing at a cancelled request) | code-level invariant: `OneActiveRequest: instance i1 is MODEL_PENDING with 0 pending turn requests` | the termination branch normalizes like `reset_instance`/`fail_request`; `terminating_an_instance_normalizes_its_execution_pointer` |
| **V-P2** | `import_response` never checked `kind`, so a compression request could be imported as a turn | the walk reached the path and succeeded (the spec requires a refusal) | the control plane refuses `kind != 'turn'`; `import_response_refuses_a_compression_request` |
| Property fix | `V2Checks` first stated "SUCCEEDED implies the recorded verdict is pass" — a **vacuous** property (an action writes that variable itself) | relaxing `Accept` still "passed" | rewritten to bind the **observed check result**; relaxing it is then immediately refuted |
| **Seven claims that could not fail** (D-219) | six variables no action ever changed (`V2Compress`' `lost`/`uncovered`, `V2Daemon`'s `pruned`/`drift`/`shrank`, `V2Checks`' `rewound`), carrying `NoEntryIsEverLost`, `CoverageNeverLifted`, `NoResyncInThisVersion`, `ReceiptsAreStable`, `LogMonotone` and `RoundsAreMonotone`; and `V2Compress`' `RequestClosesOnce`, entailed by `TypeOK` | with `ReopenClosedRequest = TRUE` the **old** `RequestClosesOnce` verifies (`No error has been found`) while the rewritten form is refuted in the same configuration | each variable is written by one counterfactual step and each claim has a control that refutes it (seven configurations); the audit rule fails a variable no action writes (`review/verification_catalogue.py`, control `--tla DIR`) |
| **Three modules nobody could refute** (D-220) | `V2Artifact`, `V2Wait` and `V2Grants` were the spec of no counterexample configuration: every claim they carry was stated and enumerated exhaustively, and none was shown refutable. `V2Wait`'s own refutation had existed — the fix ledger records V-W1 against `MC_wait_contract.cfg`, "renamed since to `MC_wait.cfg`" | with `CloseWithoutAnswer = TRUE` (both non-drain exits close a wait and answer nothing) TLC reports `ResolvedWaitIsAnswered` violated — the property the ledger names for V-W1, refutable again | one counterfactual constant and one control per module (`MC_artifact_gc_ignores_references.cfg` → `GcClaimsOnlyUnreferencedLive`, `MC_wait_closes_without_answering.cfg` → `ResolvedWaitIsAnswered`, `MC_grants_stale_offered_surface.cfg` → `OfferedToolsAreAuthorized`); `review/verification_catalogue.py` now fails a `V2*.tla` that is the spec of no `cfg:spec` pair |
| **Ten claims the mapping never named** (D-222) | `verification/README.md` is the property-by-spec mapping the report sends a reader to; ten of the 148 marked claims were absent from it — all four of `V2Retention`'s substantive claims (`EvictionOnlyUnderTheGuards`, `NoReferenceToEvictedFact`, `EvidenceIsNeverEvicted`, `OnlyOldFactsAreEvicted`), `V2Grants`' four `TypeOK*` components and `V2Wait`'s `SatisfiedHoldsConditions` and `AnswerImpliesResolved` | — (a documentation gap, not a model defect: every one was listed by a configuration and checked) | all ten are named in the mapping beside the module that marks them, and `review/verification_catalogue.py` fails a marked claim the mapping never mentions (control: `--mapping` on a copy with one name deleted) |
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
| A21 | the user adjusts an instance directly | — | sample tests for the single-writer context, plus the live TUI runs (`review/dogfood/tui.py` for the composer path, `cancel.py`/`job_identity.py` for a direct input to a member, `approval.py` for the approvals box) |
| A22 | wait cycles and timers | `V2Wait` (including "a parked PENDING is eventually closed or superseded") | the graph algorithm of cycle detection itself |
| A23 | a result arrives before the wait is registered | `V2Wait` evaluation at registration plus code-level `ResolvedWaitIsAnswered` | — |
| A24 | a late result after a reset | `V2Control::NoReceiptAcrossEpochs` | — |
| A25 | MCP approval / cancellation / unknown outcome | model `NoEffectBeforeApproval`; `V2Approval` (D-225): the `expires_at` window, a final decision, no pending approval after the operation closes; code-level approval finality, pending approvals only on PREPARED operations, no effect after a denial | MCP transport and tool surface |
| A26 | Skills permissions | — | real symlink/registration-root evidence |
| A27 | heterogeneous providers cooperating | — | real two-sided message evidence |
| A28 | disconnect, slow client, reconnect | all 10 `V2Daemon` properties plus code-level receipt stability, replay inertia and append-only logs | the real socket layer (covered by the daemon tests) |
| A29 | session isolation and a shared project | the single-session constraint in `V2Control` | shared-directory authorization |
| A30 | artifact and DB write boundaries | all 8 `V2Artifact` properties plus code-level "LIVE has bytes" | real power loss |
| A31 | write failure / disk full | — | `StorageFull` classification and real SQLite FULL injection |
| A32 | very large history measurement | pure-function seamless paging reconstruction (the coordinate semantics of readback) | the performance numbers themselves |
| A33 | two daemons / stale lock | — | real lock and second-daemon evidence |
| A34 | incompatible schema | `V2Store` (a foreign database is never stamped as ours, a refusal writes nothing, a half-initialized database is completed rather than stranded) plus the refuted control `MC_store_adopt.cfg` | byte-level evidence in `review/dogfood/boundary.py` and migration sample tests |
| A35 | goal deadline | the deadline gates in `V2Control` (`AdmissionGate` plus a hard dispatch refusal) | real clock boundaries |
| A36 | install / init / doctor / cleanup | — | CLI and cleanup evidence |

Subtotal: **26 items** have a non-empty "formal coverage" entry (A01–A04, A06–A11, A13, A16–A20, A22–A25,
A28–A30, A32, A34, A35) and the remaining **10** (A05, A12, A14, A15, A21, A26, A27, A31, A33, A36) have
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
5. ~~A modeled rule without an implementation (D-192)~~ — **implemented in D-245**: `control::prune_history`
   drops ordinary history at a session's boot under the four guards this model pins, and its test
   `retention_sweep_keeps_the_models_invariants` exercises each guard *and* its counterfactual over the real
   command sequence (a pending wait keeps a delivery, satisfying the wait lets the next sweep take it; the
   log's head survives an aging that would otherwise take it; `[retention] archived_days` stays unapplied —
   one session per state root, A33 — and `doctor` says so). What remains unproven is the usual ceiling: the
   model is not a refinement proof, so the correspondence is bounded-enumeration-plus-test, not a theorem.
6. **Concurrency**: `Control::submit` is serialized on a single connection (a single writer) and the model
   does not cover interleavings across connections; the daemon's concurrent read and write connections appear
   only in A28's structural statement that a slow client cannot block the writer, without an exhaustive
   interleaving.
7. **Pure-function layer**: the paging arithmetic has a Kani machine proof and the proven `page_span` is the
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
make verify-model-all     # exhaustive configurations for the fifteen surfaces (seconds to ~2 min;
                          # the task, grants and authority models are the slow ones)
make verify-model-counterexamples  # the forty-eight negative controls, each must be refuted
make verify-model-wide    # wide control-plane configuration (best effort: 275M states / 11 m in the historical
                          # run; a one-hour bounded attempt on 2026-09-27 did not reach a verdict)
make check                # fmt + clippy -D warnings + 23 suites (including the two code-level layers)
make verify-kani          # Kani proofs for the paging arithmetic (~16 s; the toolchain is at ~/.cargo/bin)
```

Any one of the following invalidates the conclusions above and requires a re-run and an update:

- a spec file changes (a property weakened, an action relaxed) — TLC only proves the spec as it was;
- a code-level coverage assertion fails (the walk no longer reaches some key state);
- the checker-sensitivity test fails (the checker can no longer find deliberate breakage);
- the gate or `verify-model-*` reports a violation;
- a new deviation is found (for example another state the model considers impossible but the code reaches).
