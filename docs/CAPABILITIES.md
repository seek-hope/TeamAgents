# Capabilities and their evidence

This page is the **map, not the ledger**: one row per capability, the strongest check behind it, what that check
measured as the capability's *boundary*, and what still has no evidence. Every row points into a ledger that
carries the detail, and nothing here overrides one of them. Current behavior was checked against source
02be286d through D-474 on 2026-10-22; historical measurement dates below have not been renewed. The prioritized candidate
queue lives in [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md), not in duplicate feature lists here.

| For | Read |
|---|---|
| what the product must do and whether it does it | [ACCEPTANCE.md](ACCEPTANCE.md) — the A01–A36 matrix, item by item, with the test, probe or measurement that drove it |
| why a design is what it is, and what was measured | [DECISIONS.md](DECISIONS.md) — every confirmed change, its evidence and its ceiling |
| what the formal models prove, and what they do not | [../verification/REPORT.md](../verification/REPORT.md) and [../verification/README.md](../verification/README.md) |
| the Terminal-Bench 2.1 evidence and how to re-run it | [../review/benchmark/README.md](../review/benchmark/README.md) |
| what the benchmark failures are made of, product-side | [../review/benchmark/CAPABILITY.md](../review/benchmark/CAPABILITY.md) |

**Evidence classes.** A *test* runs in `make check` (offline, no credentials). A *probe* is a re-runnable script
under `review/dogfood/` or `review/benchmark/`; some are credential-free local/fake-service checks and
others need a real model or external service. Read the probe's declared mode before treating it as live evidence. A *model* is
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
| Isolation by default, and a refusal instead of a host fallback | `V2Isolation.tla` (`UnavailableBackendNeverRuns`); `make check-nobwrap`, `make check-broken-sandbox`; `review/nobwrap_path.py` | the Docker and `sbx` backends are selectable and fail-closed (D-369, D-478); neither has a dogfood probe of the weight the bubblewrap ones carry, and `sbx` is not installed on this host | end-to-end Docker and `sbx` probes |
| Visibility is not delivery, and history is private | A05; `V2History.tla` (`InstancesReadOnlyTheirOwn`, `SystemNeverReads`) | the user's "may read any" is the absence of a restriction and so has no state half to break | — |
| A call runs because a *binding* authorizes it | `v2_mcp::skill_call_without_the_binding_fails_honestly`; `V2Grants.tla`, `V2Authority.tla` | the binding gate's own refusal message is pinned by a test, so it cannot be reworded for convenience | — |
| An approval binds one concrete operation, argument hash and permission revision | `review/dogfood/approval.py` (real TUI, approve and deny); `V2Approval.tla` (`NoLateEffect`, `ApprovalDecisionIsFinal`) | an approval is used up by that single dispatch; a leftover is expired automatically or denied by hand | — |
| Task-scoped capability delegation | `core/src/v2/control.rs` capability tests; `V2Capabilities.tla`, `V2Grants.tla`, `V2Authority.tla`; D-455 | user-issued delegable parent, attenuated non-delegable children, task/instance expiry and parent revocation; otherwise WAITING_CAPABILITY | matched real-coding intervention/cost measurement |
| Automatic operation approval review | `engine/src/v2/reviewer.rs`, `engine/tests/v2_driver.rs`; D-458 | one concrete operation only, no capability approval; failed review leaves it pending; substring hard denials are not a complete safety policy | reviewer usage in goal accounting; adversarial and real-service evidence |
| Required checks gate *settlement* | `review/dogfood/checks.py` (three rounds, then the goal parks `BLOCKED` with the failing check named); `V2Checks.tla` | **the gate cannot stop a turn that never claims to be done** (D-400) and is inert where the agent never calls `finish` (D-403); a **wrong** check is worse than none (D-400: `winning-avg-corewars` 3/3 at 966 s became 1/3 at 3,649 s because the check named a path the agent did not use) | D-461 provides idle-unsettled diagnostics with explicit recovery, not automatic settlement; independent verification can run pre-authorized checks mid-turn via verify_goal |
| Independent verification of a claim | `v2_daemon::an_independent_goal_does_not_settle_on_its_producers_own_passing_round`; `V2Checks.tla` (`IndependenceIsNotSelfVerified`) with `MC_checks_self_verified.cfg` refuted (D-393) | **provenance, not strength**: producer and verifier run the same commands under the same authority (D-398), and no second agent can recover a criterion the environment does not reveal (D-401: 0/3, all failing the same unobservable convention) | — |

## 3. Execution, verification and the clock

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| The phase machine, and recovery driven by persisted location | the `engine` suite; `v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart`; `V2Control.tla` | model and tool waits happen outside transactions, by design | — |
| A job runner that outlives an exit and then retires | A12; `review/runner_cost.py` (idle cost 0.40 % → 0.00 % of a core), `review/host_cleanup.py` (the census and the classes it may stop) | an *unsettled* job's runner keeps waiting by design, at ~0 CPU | — |
| A ceiling on one model tool call (`[limits] max_tool_seconds`) | `v2_driver::the_configured_ceiling_bounds_a_model_picked_tool_timeout` (the job's own `deadline_ms`: a 3600 s ask capped to 60 s, an uncapped arm left at 3600 s, a 30 s ask untouched), `tools::bounded_timeout_tests` | it is a ceiling on what a **model** may ask for, not a shorter default: a long legitimate command is cut at the ceiling and must be split by the model; a user's own `[[checks]]` timeout is never clamped (D-415) | a measurement of whether the cut helps |
| Recovery-oriented errors instead of bare refusals | D-402 (`winning-avg-corewars`: 53 invented tool calls → 0 and 255 → 138 operations once the answer names the offered tools); D-404 (the same in codemode) | measured as operations and invented calls, not as a score | — |
| The criterion arrives **before** the work: a goal's required checks are briefed at the turn's start | `control::the_check_briefing_names_the_required_checks_once_and_only_for_a_live_goal`, `v2_driver::a_goal_with_required_checks_is_told_them_before_the_first_request`; `V2Checks`' `BriefingIsOnce` + `BriefingIsForAGoalWithChecks`, refuted by a control that briefs a goal requiring nothing (D-418) | the text is composed by the runtime from the goal's own stored limits and written at most once per goal; the A/B is a **null**: the gate wording measured 0/9 against 3/9 and the information wording 2/9 against 0/9, with receipts proving the note fired 9/9 in each treatment arm and 0/9 in each control — opposite directions, and the control arm's own score moved 3/9 → 0/9 between runs with an unchanged binary, so nine trials per arm cannot separate the briefing from the noise. The run's durable outcome is the **conversation-shape bug** it exposed (the boundary's synthetic answer fired for a call whose operation was still open, whose real receipt then landed as a second tool result in a request the provider refuses — fixed, with a test that fails without the fix) (D-418) | nothing: the briefing stays as information, not as evidence of an improvement |
| A wrap-up note when a goal's deadline is close | `control::the_deadline_warning_is_sent_once_and_only_before_a_live_deadline`, `v2_driver::a_goal_close_to_its_deadline_is_told_once_to_wrap_up`; `V2Control.tla` (`AtMostOneWarning`, `WarnIsTimely`) with two refuted controls; `review/benchmark/deadline_wrapup_ab.py` | **delivered only at a step boundary**: on the 12 timeout-prone tasks, bare 0/12 against 1/12 with a 13-minute deadline, and the runs that miss it are inside one long tool call — the same wall D-381's 12-hour budget and D-400's gate hit from other sides (D-410) | bound a single tool call's wall clock so a goal near its deadline returns to the model more often |
| A deadline is a cut, not a verdict | D-397: **10 of the 33 deadline trials were graded 1.0 anyway**; `review/benchmark/verifier_health.py` separates a verifier that never ran | more time does not convert the rest: a 12-hour budget did not raise the score (D-381) | — |

## 4. Context and cognition

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| Compaction that survives a restart, and never blocks a turn | `V2Compress.tla` (eight claims); `v2_driver::failed_summaries_fall_back_uncompressed_and_break_after_three` | a summary that cannot be written leaves the originals in the view and trips a breaker after three attempts | — |
| Stable prompt-cache prefix and cache telemetry | `core/src/kernel/mod.rs`, provider-body tests in `engine/tests/providers_fake.rs`, `engine/src/v2/exec.rs`; D-459 | provider-reported hit/miss/write, frozen L1 frontier until compaction, Anthropic breakpoints and official OpenAI session key; no report means unknown, not zero hits | priced cost/attribution, reviewer accounting and measured post-change service behavior |
| codemode: one tool, a QuickJS sandbox, only the script's own output in context | `review/codemode` tests in the `engine` suite; `V2Codemode.tla` with a refuted leak control; the `pre_tool` veto, structured results and the persistent store (D-374, D-376) | MCP-only synchronous host bridge, not native-file/shell composition or parallel MCP execution; image() yields markers. The frequently quoted token saving is Pi's measurement, not a TeamAgents result | a product-side token measurement on a fixed task |
| An image reaches a vision model, and a model without support is told so | `v2_daemon::view_image_reaches_the_wire_as_a_part_only_for_a_model_that_declares_images`; `V2Images.tla` (`NoImagesWithoutSupport`, `PartsOnlyFromLoadedBytes`) with a blind-model control refuted; live: a red square answered, 228 against 44 prompt tokens | priced at ~184 / 177 / 645 prompt tokens for 16² / 256² / 1024² pixels (measured); on its motivating task the structural barrier is gone but the capability gap remains (0/3 at 1,800 s, D-394) | — |
| Durable memory for facts that exist nowhere on disk | `V2Memory.tla` (`NotesAreAppendOnly`, `TheStoreRefusesAtTheCap`, `RecallIsBounded`) with an eviction control refuted; the fixture measurement **0/3 → 3/3** (D-406), whose control twice wrote an explicit "choosing one would be an unverifiable guess" | **no measurable gain when the state is already on disk**: on a real task a second session went 2/3 with memory and 2/3 without (D-409), so a note pays only for what artifacts do not carry | a long project whose facts never become files |
| Skills and MCP services, over both transports | `review/dogfood/skills.py`, `mcp.py`, `mcp_http.py`; `cli::mcp_list_names_the_services_and_their_tools`; `mcp add`/`mcp remove` edit exactly one binding block (D-399, D-408); `mcp login`/`logout` sign in to an OAuth HTTP service (discovery, dynamic registration, PKCE with a state, loopback or pasted redirect, a 0600 credential file, refresh on expiry or a 401) with `V2Oauth.tla`'s five claims verified, five controls refuted and `engine/tests/mcp_oauth.rs` driving the real binary against a fake authorization server (D-412) | the nearest trusted project config is read (D-434); **no vendor's OAuth server was exercised**, so the sign-in is accepted against the specifications and a fake, not against a live service | MCP management lives in the developer CLI; `status --json` reports the running session's bindings (D-445) |
| User hooks: a `pre_tool` veto and a `notify` stream | `review/dogfood/hooks.py`, with a broken-hook negative control | a veto stops that call; it does not stop the turn | — |

## 5. The product surface

| Capability | Strongest evidence | Measured boundary | Open |
|---|---|---|---|
| One daemon per state root, a headless client, and a TUI | `review/dogfood/tui.py`, `tui_panels.py`, `tui_reconnect.py`, `input_latency.py`; `make pty`; `V2Coordinator.tla`, `V2Concurrency.tla` | rendering and mouse hit-testing share one geometry module, so a panel that draws must be the panel that is clicked | — |
| Readable conversation Markdown and compact tool results | `tui/tests/conversation.rs`, PTY smoke; Kani transcript-window proof | shared live/history renderer, explicit streaming, Unicode-cell wrapping, four/forty-line previews | full tool output stays in stored history |
| Mermaid, SVG, DOT and local image previews | `tui/tests/graphics.rs`, `tui/scripts/pty_graphics_smoke.py`; Kani media-clipping proof | native Kitty PNG placement, Unicode/block fallback, author-scoped local references, Ctrl+G source toggle | bounded static previews; remote URLs remain links; no universal diagram-language parity |
| Conversation completion, code colors and long-history media | `tui/tests/parity.rs`, `tui/src/graphics_tests.rs`, `tui/src/graphics_worker_tests.rs`; `V2Presentation.tla` and `V2DisplayWorker.tla`; [maturity report](../review/maturity-2026-10-11/REPORT.md) | seven completion-menu commands plus separately recognized /edit; seven grammar families and diff; twenty distinct images including eviction/revisit; terminable isolated display processes with source fallback | wider terminal/OS coverage; these are synthetic/local-runtime checks, not model-success evidence |
| Input attachments and pending revisions | `core/tests/input_queue.rs`, `engine/src/v2/attachments_tests.rs`, `engine/tests/cases/inputs.rs`, maturity workflow/PTY probes; `V2InputQueue.tla` | engine-owned confined immutable snapshots, explicit draft submission, revision-checked edit/cancel, original audit rows and native image parts | real-provider image acceptance and richer follow-up semantics |
| External composer editor | `tui/src/editor.rs` success/failure test; routing in `tui/src/v2app.rs` and `tui/src/main.rs` | VISUAL/EDITOR, Ctrl+E or draft /edit suffix, no implicit send, error restores draft; /edit is absent from completion | editor crash/resize/reconnect and graphical-terminal integration coverage |
| Local export and browser search | `engine/src/v2/session_export_tests.rs`, `engine/tests/session_browser.rs`, `tui/tests/sessions.rs`, maturity PTY probe | complete bounded read snapshot in Markdown/JSON, references retained, no overwrite, real SIGINT cleanup; search preview is a snippet | HTML export and message-level branches |
| Sessions: registry, fork, rename, restore, search, TUI history/switch/delete | `V2Sessions.tla` (`RegistryIsInjective`, `ArchiveOnlyWhenIdle`) with three refuted controls; `V2SessionUi.tla` with four refuted controls; `engine/tests/session_browser.rs`, `tui/tests/sessions.rs`, `review/dogfood/tui_sessions.py` | fork requires no live source daemon; no message-level tree; memory belongs to the base, not one named session (D-405) | project/session selection ergonomics and message-level branches |
| Read-only work diagnostics and explicit recovery | engine/src/v2/diagnostics/tests.rs; engine/tests/v2_daemon.rs restart/send/cancel cases; tui/tests/minimal.rs frames | D-461 option A: current-epoch persisted facts, retained reasons, wait/timer and current-operation detail; no model calls or state changes on observation | real-user effectiveness and large-team polling measurements; automatic reminders are excluded. The readiness rule is shared with the driver (`engine/src/v2/readiness.rs`), so the two cannot drift |
| Interventions: pause, resume, terminate, cancel a task | `review/dogfood/lifecycle_run.py`, `cancel.py`; D-68's headless path | **cancelling a task is not stopping its work**: the assignee's command runs until its own timeout unless the instance is terminated (D-88) | — |
| Automations: a schedule that starts a goal on its own | `V2Schedule.tla` (`NoDuplicateRun`, `AtMostOneRunPerAutomation`, `DisabledNeverRuns`, `OnlyDueSlotsRun`) with four refuted controls | safety only: a run need not ever happen | — |
| Workspace policies: shared, isolated, git worktree | `workspace::tests::shared_and_isolated_roots`, `worktree_lifecycle_and_guards`; `review/dogfood/workspace.py` | retirement refuses anything with uncommitted or unmerged work, and says why | — |
| Install, init, doctor, cleanup, reopen | `engine/tests/install.rs`; `review/dogfood/boundary.py`; `make release-rehearsal`; D-451 installed-artifact probe | the D-451 pinned-source musl archive was installed and exercised locally; v0.2.0 is the first public release (Linux x86_64), and source builds remain available | a separately authorized release; untested platforms |

## 6. Evaluation standing: three benchmarks, DeepSeek Flash

The headline rows and the rules that make them readable are in
[../review/benchmark/README.md](../review/benchmark/README.md); this is the short form. The three measure
different things on purpose — a single benchmark's tasks are a shape a product can be tuned into (D-419).

| Benchmark | What it grades | Sample | Result |
|---|---|---|---|
| Terminal-Bench 2.1 | container tasks, the task's own tests | 89 tasks × 3, per-task budgets (D-389/D-390) | **210/267 = 78.7 %** [0.733, 0.831]; 210/258 = 81.4 % over the 86 evaluable tasks |
| SWE-bench Verified | real GitHub issues, the **repository's own tests**, official harness | seeded random 12, retrieval **blocked** (D-419) | **8/12 = 66.7 %** [0.39, 0.86] |
| τ³-bench (retail / airline) | a **conversation with a simulated user**, graded from the **database state** | the **full splits** — 114 retail and 50 airline dialogues, one attempt each, DB check recomputed from the product's own calls (D-420, D-431) | **96/114 = 84.2 %** · **41/50 = 82.0 %** |

**Rule 1: a benchmark whose fixes are public must be run with the retrieval channel closed, and the guard's
receipt read.** SWE-bench's first three runs (13/14, 12/12, 12/12) were void: near-verbatim copies of the upstream
patches, fetched with `pip download <pkg>==<later>` and `https://github.com/<repo>/commit/<sha>.patch` (D-419).
Patch-vs-gold similarity is the cheap probe, and a run's trajectory must be collected to be auditable at all.

**Rule 2: read the check's own receipt before attributing a result to it** (D-395: an acceptance check that never
ran because the image ships no `python3`; D-410: a wrap-up note no model ever received; D-416: a measurement whose
binary predated the instrument).

The results above are historical task outcomes, not Pi comparisons or measurements of every later source
revision. The corrected tau3 full-split results supersede its small pilot; the invalid retrieval-contaminated
SWE-bench runs remain invalid. The benchmark ledger preserves those corrections and the underlying receipts.

**How to read it.** One full-set run is a point estimate with a **±5-task band** (D-388). Each task runs with its
**own** authored budget (48 × 900 s, 17 × 1,800 s, 13 × 3,600 s, plus three longer) because harbor does; the
harness never shrinks a real model's context window; and the adapter calls the same system-dependency setup every
built-in harbor agent does (D-382). Three tasks are excluded only from the **adjusted 258-trial denominator**, not from the all-task 267:
 `qemu-alpine-ssh` and `qemu-startup` (their images' mirror rot — the verifier cannot install anything, so the
0 is not the agent's), and `pytorch-model-cli` (an under-determined specification: it grades the reference
implementation's `/255` convention while the only criterion the environment offers favours the other one, D-401).
`mteb-retrieve` was excluded here for a while (D-423) and is back in the denominator: a later trajectory showed the
prompt it needs is discoverable from `mteb`'s own model metadata, which is what the instruction says to use — the
earlier failures were a self-check that overwrote the correct answer (D-425).

**Failure diagnoses are task-specific.** The recorded Terminal-Bench distribution was 55 tasks at 3/3,
17 at 2/3, eight at 1/3 and nine at 0/3; 40 trials ended in exceptions (33 deadline cuts and seven nonzero
exits). D-414 measured model-dominated and tool-dominated cases separately. D-416 found slow streaming,
not a stalled connection, in its sampled long requests; this is not a claim that providers never stall.
D-424 found that larger prompts were a billing issue rather than the dominant latency source in its sample.
These results justify measuring cadence and cost, not assuming that a shorter window, longer timeout,
higher effort or more agents will improve every task. Keep native windows in all real-model evaluations.

## 7. Remaining evidence and candidate work

[The daily-coding queue](PRODUCT-COMPARISON.md#2-candidate-improvements-in-priority-order) is the single
priority list. The major unclosed evidence boundaries are:

- Matched Pi coding outcomes, manual intervention and complete cost: existing A/B/C arms are internal
  TeamAgents ablations, not comparator trials.
- Real selected provider/image/OAuth/Docker paths, full skill support resources and user-home development
  environments; local serialization and scripted-provider tests establish different facts.
- Adversarial review of auto-approval and other boundaries; broader graphical terminal/platform coverage.
- Exact-artifact installation and update acceptance for the authorized v0.2.0 release (D-477).
- Long-horizon memory where facts are not already files; D-406's fixture and D-409's real-task null result
  do not establish general benefit.
- Q16's collaboration-success form: the narrower paired latency result is confirmed in D-264, not a
  general end-to-end success improvement. Whether it satisfies Q16's wording remains a user decision.

MCP management in the CLI and absence of automatic cross-session memory are product choices, not missing
promised TUI features. External editing, capability delegation and cache telemetry are implemented; their
coverage and remaining limitations are separated from absent features above.
