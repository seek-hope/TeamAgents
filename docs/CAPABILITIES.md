# Capabilities and their evidence (2026-10-04)

This page is the **map, not the ledger**: one row per capability, the strongest check behind it, what that check
measured as the capability's *boundary*, and what still has no evidence. Every row points into a ledger that
carries the detail, and nothing here overrides one of them.

| For | Read |
|---|---|
| what the product must do and whether it does it | [ACCEPTANCE.md](ACCEPTANCE.md) — the A01–A36 matrix, item by item, with the test, probe or measurement that drove it |
| why a design is what it is, and what was measured | [DECISIONS.md](DECISIONS.md) — every confirmed change, its evidence and its ceiling |
| what the formal models prove, and what they do not | [../verification/REPORT.md](../verification/REPORT.md) and [../verification/README.md](../verification/README.md) |
| the Terminal-Bench 2.1 evidence and how to re-run it | [../review/benchmark/README.md](../review/benchmark/README.md) |
| what the benchmark failures are made of, product-side | [../review/benchmark/CAPABILITY.md](../review/benchmark/CAPABILITY.md) |

**Evidence classes.** A *test* runs in `make check` (offline, no credentials). A *probe* is a re-runnable script
under `review/dogfood/` or `review/benchmark/` that needs a real model and sometimes a real service. A *model* is
a TLA+ specification (`make verify-model-all`, `make verify-model-counterexamples`) or a Kani proof
(`make verify-kani`). A *measurement* is a dated number whose raw output stays in the tree. Where a claim rests on
no more than a decision's own wording, the row says so.

## 1. The kernel: one writer, one transaction, durable answers

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| One entry point per team transaction (ingest → validate → reduce → persist, one SQLite transaction per session) | the `core` suite; `V2Control.tla` (`NoTurnWithoutWork`, `AdmissionGate`, `StaleExecutorRejected`) | identity, operation ids and permission revisions are never taken from a model or a client field, by construction | — |
| Durable state with a format stamp, and migration across versions | `store::open_refuses_unstamped_foreign_and_wrong_version`, `open_migrates_the_previous_schema_version`; `V2Store.tla` (`NoForeignAdoption`) | a foreign database is refused, never adopted (the pre-D-87 adoption is a refuted control); `synchronous = FULL` costs wall clock, which the large-history probe prices | — |
| An unknown external outcome is never replayed on a guess | `review/dogfood/unknown_outcome.py` (kill daemon and runner mid-command: `OUTCOME_UNKNOWN`, the task parks, the command never runs again); `V2Jobs.tla` | the price is that a person must re-dispatch the command | — |
| Disk full stops new dispatch, reports the loss and parks | `v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking`; `V2DiskFull.tla` with its four refuted controls | **no live evidence by construction**: filling a filesystem needs mount privileges this repository does not assume, and the product deliberately exposes no store-level fault hook | — |
| A very large history stays workable | `cargo run --offline --manifest-path engine/Cargo.toml --example load_probe` re-measured 2026-10-03: 1.26M-token history, append p50 6.1 ms, request build p50 279 ms, peak RSS 102 MiB | the history is synthetic text, so the numbers describe local overhead and never model capability | — |

## 2. Information, authority, and what may be proved

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| Isolation by default, and a refusal instead of a host fallback | `V2Isolation.tla` (`UnavailableBackendNeverRuns`); `make check-nobwrap`, `make check-broken-sandbox`; `review/nobwrap_path.py` | the Docker backend is selectable and fail-closed (D-369) but has no dogfood probe of the weight the bubblewrap ones carry | an end-to-end Docker probe |
| Visibility is not delivery, and history is private | A05; `V2History.tla` (`InstancesReadOnlyTheirOwn`, `SystemNeverReads`) | the user's "may read any" is the absence of a restriction and so has no state half to break | — |
| A call runs because a *binding* authorizes it | `v2_mcp::skill_call_without_the_binding_fails_honestly`; `V2Grants.tla`, `V2Authority.tla` | the binding gate's own refusal message is pinned by a test, so it cannot be reworded for convenience | — |
| An approval binds one concrete operation, argument hash and permission revision | `review/dogfood/approval.py` (real TUI, approve and deny); `V2Approval.tla` (`NoLateEffect`, `ApprovalDecisionIsFinal`) | an approval is used up by that single dispatch; a leftover is expired automatically or denied by hand | — |
| Required checks gate *settlement* | `review/dogfood/checks.py` (three rounds, then the goal parks `BLOCKED` with the failing check named); `V2Checks.tla` | **the gate cannot stop a turn that never claims to be done** (D-400) and is inert where the agent never calls `finish` (D-403); a **wrong** check is worse than none (D-400: `winning-avg-corewars` 3/3 at 966 s became 1/3 at 3,649 s because the check named a path the agent did not use) | a check the agent can run before declaring done; a stall detector |
| Independent verification of a claim | `v2_daemon::an_independent_goal_does_not_settle_on_its_producers_own_passing_round`; `V2Checks.tla` (`IndependenceIsNotSelfVerified`) with `MC_checks_self_verified.cfg` refuted (D-393) | **provenance, not strength**: producer and verifier run the same commands under the same authority (D-398), and no second agent can recover a criterion the environment does not reveal (D-401: 0/3, all failing the same unobservable convention) | — |

## 3. Execution, verification and the clock

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| The phase machine, and recovery driven by persisted location | the `engine` suite; `v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart`; `V2Control.tla` | model and tool waits happen outside transactions, by design | — |
| A job runner that outlives an exit and then retires | A12; `review/runner_cost.py` (idle cost 0.40 % → 0.00 % of a core), `review/host_cleanup.py` (the census and the classes it may stop) | an *unsettled* job's runner keeps waiting by design, at ~0 CPU | — |
| A ceiling on one model tool call (`[limits] max_tool_seconds`) | `v2_driver::the_configured_ceiling_bounds_a_model_picked_tool_timeout` (the job's own `deadline_ms`: a 3600 s ask capped to 60 s, an uncapped arm left at 3600 s, a 30 s ask untouched), `tools::bounded_timeout_tests` | it is a ceiling on what a **model** may ask for, not a shorter default: a long legitimate command is cut at the ceiling and must be split by the model; a user's own `[[checks]]` timeout is never clamped (D-415) | a measurement of whether the cut helps |
| Recovery-oriented errors instead of bare refusals | D-402 (`winning-avg-corewars`: 53 invented tool calls → 0 and 255 → 138 operations once the answer names the offered tools); D-404 (the same in codemode) | measured as operations and invented calls, not as a score | — |
| The criterion arrives **before** the work: a goal's required checks are briefed at the turn's start | `control::the_check_briefing_names_the_required_checks_once_and_only_for_a_live_goal`, `v2_driver::a_goal_with_required_checks_is_told_them_before_the_first_request`; `V2Checks`' `BriefingIsOnce` + `BriefingIsForAGoalWithChecks`, refuted by a control that briefs a goal requiring nothing (D-418) | the text is composed by the runtime from the goal's own stored limits and written at most once per goal; **its benefit is unmeasured** — the A/B (D-400's three tasks, both arms carrying the same check, control = the pre-D-418 binary) is pre-registered and running | the A/B's number |
| A wrap-up note when a goal's deadline is close | `control::the_deadline_warning_is_sent_once_and_only_before_a_live_deadline`, `v2_driver::a_goal_close_to_its_deadline_is_told_once_to_wrap_up`; `V2Control.tla` (`AtMostOneWarning`, `WarnIsTimely`) with two refuted controls; `review/benchmark/deadline_wrapup_ab.py` | **delivered only at a step boundary**: on the 12 timeout-prone tasks, bare 0/12 against 1/12 with a 13-minute deadline, and the runs that miss it are inside one long tool call — the same wall D-381's 12-hour budget and D-400's gate hit from other sides (D-410) | bound a single tool call's wall clock so a goal near its deadline returns to the model more often |
| A deadline is a cut, not a verdict | D-397: **10 of the 33 deadline trials were graded 1.0 anyway**; `review/benchmark/verifier_health.py` separates a verifier that never ran | more time does not convert the rest: a 12-hour budget did not raise the score (D-381) | — |

## 4. Context and cognition

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| Compaction that survives a restart, and never blocks a turn | `V2Compress.tla` (eight claims); `v2_driver::failed_summaries_fall_back_uncompressed_and_break_after_three` | a summary that cannot be written leaves the originals in the view and trips a breaker after three attempts | — |
| codemode: one tool, a QuickJS sandbox, only the script's own output in context | `review/codemode` tests in the `engine` suite; `V2Codemode.tla` with a refuted leak control; the `pre_tool` veto, structured results and the persistent store (D-374, D-376) | the frequently quoted "≈40 % fewer prompt tokens" is **Pi's own measurement of Pi's codemode**, not measured here | a product-side token measurement on a fixed task |
| An image reaches a vision model, and a model without support is told so | `v2_daemon::view_image_reaches_the_wire_as_a_part_only_for_a_model_that_declares_images`; `V2Images.tla` (`NoImagesWithoutSupport`, `PartsOnlyFromLoadedBytes`) with a blind-model control refuted; live: a red square answered, 228 against 44 prompt tokens | priced at ~184 / 177 / 645 prompt tokens for 16² / 256² / 1024² pixels (measured); on its motivating task the structural barrier is gone but the capability gap remains (0/3 at 1,800 s, D-394) | — |
| Durable memory for facts that exist nowhere on disk | `V2Memory.tla` (`NotesAreAppendOnly`, `TheStoreRefusesAtTheCap`, `RecallIsBounded`) with an eviction control refuted; the fixture measurement **0/3 → 3/3** (D-406), whose control twice wrote an explicit "choosing one would be an unverifiable guess" | **no measurable gain when the state is already on disk**: on a real task a second session went 2/3 with memory and 2/3 without (D-409), so a note pays only for what artifacts do not carry | a long project whose facts never become files |
| Skills and MCP services, over both transports | `review/dogfood/skills.py`, `mcp.py`, `mcp_http.py`; `cli::mcp_list_names_the_services_and_their_tools`; `mcp add`/`mcp remove` edit exactly one binding block (D-399, D-408); `mcp login`/`logout` sign in to an OAuth HTTP service (discovery, dynamic registration, PKCE with a state, loopback or pasted redirect, a 0600 credential file, refresh on expiry or a 401) with `V2Oauth.tla`'s five claims verified, five controls refuted and `engine/tests/mcp_oauth.rs` driving the real binary against a fake authorization server (D-412) | the project-level config file is not read (D-133); **no vendor's OAuth server was exercised**, so the sign-in is accepted against the specifications and a fake, not against a live service | signing in *from inside the TUI* — `/mcp` shows the state and the lever, while the flow itself stays a separate process (D-413) |
| User hooks: a `pre_tool` veto and a `notify` stream | `review/dogfood/hooks.py`, with a broken-hook negative control | a veto stops that call; it does not stop the turn | — |

## 5. The product surface

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| One daemon per state root, a headless client, and a TUI | `review/dogfood/tui.py`, `tui_panels.py`, `tui_reconnect.py`, `input_latency.py`; `make pty`; `V2Coordinator.tla`, `V2Concurrency.tla` | rendering and mouse hit-testing share one geometry module, so a panel that draws must be the panel that is clicked | — |
| Sessions: registry, fork, rename, restore, search | `V2Sessions.tla` (`RegistryIsInjective`, `ArchiveOnlyWhenIdle`) with three refuted controls; the `cli` suite | the memory store belongs to the **base**, not to one named session (D-405) | — |
| Interventions: pause, resume, terminate, cancel a task | `review/dogfood/lifecycle_run.py`, `cancel.py`; D-68's headless path | **cancelling a task is not stopping its work**: the assignee's command runs until its own timeout unless the instance is terminated (D-88) | — |
| Automations: a schedule that starts a goal on its own | `V2Schedule.tla` (`NoDuplicateRun`, `AtMostOneRunPerAutomation`, `DisabledNeverRuns`, `OnlyDueSlotsRun`) with four refuted controls | safety only: a run need not ever happen | — |
| Workspace policies: shared, isolated, git worktree | `workspace::tests::shared_and_isolated_roots`, `worktree_lifecycle_and_guards`; `review/dogfood/workspace.py` | retirement refuses anything with uncommitted or unmerged work, and says why | — |
| Install, init, doctor, cleanup, reopen | `engine/tests/install.rs`; `review/dogfood/boundary.py`; `make release-rehearsal`; `review/install_check.py` | **the published release tag is an earlier implementation**, so the install guide describes more than the newest release contains (A36's open gap) | a release of this tree |

## 6. Evaluation standing: three benchmarks, DeepSeek Flash

The headline rows and the rules that make them readable are in
[../review/benchmark/README.md](../review/benchmark/README.md); this is the short form. The three measure
different things on purpose — a single benchmark's tasks are a shape a product can be tuned into (D-419).

| Benchmark | What it grades | Sample | Result |
|---|---|---|---|
| Terminal-Bench 2.1 | container tasks, the task's own tests | 89 tasks × 3, per-task budgets (D-389/D-390) | **210/267 = 78.7 %** [0.733, 0.831]; 210/258 = 81.4 % over the 86 evaluable tasks |
| SWE-bench Verified | real GitHub issues, the **repository's own tests**, official harness | seeded random 12, retrieval **blocked** (D-419) | **8/12 = 66.7 %** [0.39, 0.86] |
| τ³-bench (retail) | a **conversation with a simulated user**, graded from the **database state** | 10 tasks, DB check, seeded (D-420) | **0/10** |

**Rule 1: a benchmark whose fixes are public must be run with the retrieval channel closed, and the guard's
receipt read.** SWE-bench's first three runs (13/14, 12/12, 12/12) were void: near-verbatim copies of the upstream
patches, fetched with `pip download <pkg>==<later>` and `https://github.com/<repo>/commit/<sha>.patch` (D-419).
Patch-vs-gold similarity is the cheap probe, and a run's trajectory must be collected to be auditable at all.

**Rule 2: read the check's own receipt before attributing a result to it** (D-395: an acceptance check that never
ran because the image ships no `python3`; D-410: a wrap-up note no model ever received; D-416: a measurement whose
binary predated the instrument).

**Where the remaining failures are.** Terminal-Bench: not reasoning depth (D-380: lower effort scored better; D-381:
a 12-hour budget changed nothing) but throughput and cadence, and the window's composition is task-dependent —
one attempt spent 98 % of its 730 s in model requests (68 of them, one 190 s), another 94–96 % of 775–841 s inside
tool calls (three asking for 300 s), with long calls *chosen by the model* (D-414). A long request is the model
streaming slowly, never a stalled connection (D-416: frame gaps ≤ 1.1 s across 232 requests). SWE-bench: four of
twelve failed with patches that take a different route than the reference fix. τ³-bench: the dialogues look right
(6–11 domain tool calls, the right tools) and the **database is still wrong** — one trajectory used
`#W2378156` where the environment only knows `W2378156`, got `Error: Order not found`, kept going with the wrong
identifier and told the user the exchange was confirmed (D-420).

**How to read it.** One full-set run is a point estimate with a **±5-task band** (D-388). Each task runs with its
**own** authored budget (48 × 900 s, 17 × 1,800 s, 13 × 3,600 s, plus three longer) because harbor does; the
harness never shrinks a real model's context window; and the adapter calls the same system-dependency setup every
built-in harbor agent does (D-382). Three tasks are **excluded from the headline denominator** and listed beside
it: `qemu-alpine-ssh` and `qemu-startup` (their images' mirror rot — the verifier cannot install anything, so the
0 is not the agent's), and `pytorch-model-cli` (an under-determined specification: it grades the reference
implementation's `/255` convention while the only criterion the environment offers favours the other one, D-401).
**Read a check's own receipt before attributing a result to it** (D-395: an acceptance check that never ran
because the image ships no `python3`).

**Where the remaining failures are.** Per task: 55 tasks 3/3, 17 tasks 2/3, 8 tasks 1/3, 9 tasks 0/3; 40 trials
(15 %) ended in an exception (33 deadline cuts, 7 non-zero exits). The surviving pattern is not reasoning depth —
lower reasoning effort scored *better* than higher (D-380) and a 12-hour budget changed nothing (D-381) — but
**throughput and cadence**: a turn is bounded by wall clock, and every runtime lever (an acceptance gate, a
wrap-up note, a deadline) is observable only where the model comes up for air (D-400, D-403, D-410).
`review/benchmark/cadence.py` then decomposed the window itself, and its composition is **task-dependent**: one
attempt spent 98 % of its 730 s in model requests (68 of them, one 190 s) while another spent 94–96 % of
775–841 s inside tool calls (three of them asking for 300 s) — so a bound on one resource cannot help the other
family, and the next experiment (`[limits] max_tool_seconds`, a ceiling on what one call may ask for) is
pre-registered against the tool-dominated family rather than assumed (D-414). The other half of that split was
measured to its end: a long model request is the model **streaming slowly** — frame gaps of at most 1.1 s and a
first frame within 0.7 s across 232 requests — so there is no stall to detect and no runtime lever on that
family (D-416).

## 7. Pending

- **Nothing on the MCP row.** The comparison's last MCP item closed with D-413: `/mcp` in the TUI shows each
  configured service and its credential state (the daemon sends the rows in its checkpoint), and the flow verbs
  stay headless on purpose — a loopback callback and a browser are a separate process's job.
- **A tool call's wall clock** — the upgrade path D-410 records and does not take: bound one call, or interrupt it
  as a deadline nears, so a goal near its deadline reaches the model more often. It is the only lever in that
  family that does not depend on the model's cadence.
- **The briefing's own measurement** — the one capability in this map whose *benefit* has no number yet: the
  pre-registered A/B (both arms carry the same check; the control is the binary from before D-418) is running.
- **Memory on a longer horizon** — D-406 measures the isolated case and D-409 the real-task case; what neither
  speaks to is a project over many sessions whose facts never became files.
- **The published release** — the newest tag is an earlier implementation, which is A36's open gap and what
  `review/install_check.py` fails on.
- **The collaboration gain's success form** — Q16's latency form is confirmed (D-264, the paired metric); the
  end-to-end success form is not, and whether the narrower gain satisfies Q16's wording is the user's call.
