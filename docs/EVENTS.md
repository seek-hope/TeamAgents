# The event log: every kind this build emits

Every control command appends to one per-session event log; the daemon answers `events` from a watermark, so a
client that reconnects reads what it missed instead of guessing (`docs/DESIGN.md` §? — "a reconnect first reads
state and its event watermark in one read snapshot and then reads events"), and `teamagents exec --json` builds
its report from the same stream (D-49/D-72: a run reports *its own* outcome, which it identifies by the events
that follow its input).

Each row carries the session, a per-session increasing sequence, a kind, a scope (usually the instance, task,
goal or operation it is about) and a JSON payload. Two things it is not: the log is **not** the state — the
session tables are authoritative and are what a reader should trust for "what is true now" — and it is **not**
transactional for the outside world: a receipt is what judges a tool call, and a service the command left behind
belongs to the user (D-41).

**Families.** `goal_*` and `check_*`/`completion_*` are the goal's life at the completion boundary; `request_*`,
`attempt_recorded` and `response_imported` are the model calls inside a turn; `task_*` and `envelope*`/`message_sent`
are delegation and messages; `operation_*` and `decision_consumed` are tool dispatch; `approval_*` and `grant_*`
are the permission surfaces; `instance_*` is lifecycle (create, spawn, reset, terminate); `context_compressed`,
`compression_*` and `inbox_drained` are context delivery; `artifact*` is staging and GC; `budget_refused` and
`goal_deadline_refused` are the admission gates.

**The table is generated.** `python3 review/event_catalogue.py --write` reads every `event(...)` call site in
`core/src` and rewrites the block below; `make hygiene` fails when the two drift (a kind emitted but not listed, a
listed kind that no longer exists, or a payload key that changed). The last column is a text search over `tui/`,
`engine/src`, `review/` and `docs/`: **observability only** means nothing in this tree reads that kind by name —
the log is still a supported way to observe it (and the probes that do assert on a kind are listed).

<!-- generated: begin -->

| Event | Scope | Payload | Emitted at | Read by (outside core/src) |
|---|---|---|---|---|
| `approval_denied` | `approval_id` | `operation_id` | `core/src/v2/control.rs:4041` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `approval_granted` | `approval_id` | `approval_id` | `core/src/v2/control.rs:4003`, `core/src/v2/control.rs:4097` | `tui/src/v2app.rs` |
| `approval_requested` | `operation_id` | `approval_id` | `core/src/v2/control.rs:3925` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/update.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `approval_reviewed` | `approval_id` | `operation_id`, `outcome`, `review` | `core/src/v2/control.rs:4043`, `core/src/v2/control.rs:4098` | `engine/tests/v2_driver.rs` |
| `artifact_abandoned` | `id` | `artifact_id` | `core/src/v2/control.rs:5153` | observability only |
| `artifact_collected` | `id` | `artifact_id` | `core/src/v2/control.rs:5554` | observability only |
| `artifact_owners_released` | `""` | `released` | `core/src/v2/control.rs:5307` | observability only |
| `artifacts_gc_claimed` | `""` | `claimed` | `core/src/v2/control.rs:5239` | `engine/tests/v2_daemon.rs` |
| `attempt_recorded` | `request_id` | `attempt_id`, `selected`, `status`, `stream`, `usage` | `core/src/v2/control.rs:2776` | `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/benchmark/cadence.py` |
| `budget_refused` | `instance_id` | `est`, `goal_id`, `known`, `max`, `request_id`, `reserved` | `core/src/v2/control.rs:2668` | `engine/src/v2/driver.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/budget.py` |
| `capability_approved` | `&subject` | `grant_id`, `request_id`, `subject`, `task_id` | `core/src/v2/control.rs:1712` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `capability_denied` | `&task_id` | `request_id`, `status`, `task_id` | `core/src/v2/control.rs:1768` | `tui/src/v2app.rs` |
| `capability_grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:616` | observability only |
| `capability_requested` | `subject` | `action`, `parent_grant_id`, `request_id`, `requester`, `resource_scope`, `subject`, `task_id` | `core/src/v2/control.rs:1357` | `tui/src/v2app.rs` |
| `check_round_registered` | `goal_id` | `checks`, `goal_id`, `round` | `core/src/v2/control.rs:4564` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/maturity-2026-10-11/workflows.py` |
| `completion_closed` | `instance_id` | `instance_id` | `core/src/v2/control.rs:4885` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs` |
| `completion_repair` | `instance_id` | `failures`, `goal_id`, `round` | `core/src/v2/control.rs:4642` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/dogfood/two_gates.py`, `review/maturity-2026-10-11/workflows.py` |
| `compression_began` | `instance_id` | `est_prompt_tokens`, `goal_id`, `request_id` | `core/src/v2/control.rs:2844` | `engine/tests/v2_driver.rs` |
| `compression_failed` | `&instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:2965` | `engine/tests/v2_driver.rs` |
| `context_compressed` | `instance_id` | `covered`, `covers_to`, `kept`, `request_id`, `summary_id` | `core/src/v2/control.rs:2929` | `engine/tests/v2_driver.rs` |
| `decision_consumed` | `&instance` | `decision_id` | `core/src/v2/control.rs:3827` | `tui/src/v2app.rs` |
| `envelopes_sealed` | `instance_id` | `envelope_ids`, `epoch`, `instance_id`, `reason` | `core/src/v2/control.rs:846` | `engine/src/v2/exec.rs` |
| `fork_reset` | `keep` | `grants_revoked`, `instances_removed`, `kept` | `core/src/v2/control.rs:5396` | `engine/src/v2/sessions.rs` |
| `goal_blocked` | `goal_id` | `detached`, `goal_id`, `reason`, `status` | `core/src/v2/control.rs:4714` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/two_gates.py` |
| `goal_cancelled` | `goal_id` | `goal_id`, `released` | `core/src/v2/control.rs:4982` | observability only |
| `goal_checks_briefed` | `goal_id` | `checks` | `core/src/v2/control.rs:4794` | `engine/tests/v2_driver.rs` |
| `goal_checks_required` | `goal_id` | `added`, `checks`, `goal_id` | `core/src/v2/control.rs:2162` | observability only |
| `goal_completed` | `goal_id` | `blocked_by`, `completion`, `detached`, `goal_id`, `reason`, `status` | `core/src/v2/control.rs:4706`, `core/src/v2/control.rs:5123` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/tests/cases/autonomy.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs`, `review/benchmark/tb_silence.py`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/dogfood/two_gates.py`, `review/dogfood/unknown_outcome.py` |
| `goal_created` | `id` | `goal_id` | `core/src/v2/control.rs:2091` | observability only |
| `goal_deadline_refused` | `instance_id` | `goal_id`, `kind`, `request_id` | `core/src/v2/control.rs:2538`, `core/src/v2/control.rs:2813` | `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/deadline.py` |
| `goal_deadline_warning` | `goal_id` | `goal_id`, `seconds_left` | `core/src/v2/control.rs:4844` | `engine/tests/v2_driver.rs`, `review/benchmark/deadline_wrapup_ab.py` |
| `grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:559` | observability only |
| `grant_revoked` | `id` | `cascade`, `grant_id`, `revision` | `core/src/v2/control.rs:649` | observability only |
| `history_pruned` | `""` | `compacted`, `days`, `deliveries`, `events` | `core/src/v2/control.rs:5508` | observability only |
| `inbox_drained` | `instance_id` | `applied`, `sealed` | `core/src/v2/control.rs:1274` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `input` | `instance_id` | `applied`, `envelope_id` | `core/src/v2/control.rs:2256` | `tui/scripts/pty_v2_smoke.py`, `tui/src/queue.rs`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/providers/anthropic.rs`, `engine/src/providers/responses.rs`, `engine/src/v2/attachments.rs`, `engine/src/v2/diagnostics/tests.rs`, `engine/src/v2/exec.rs`, `engine/src/v2/session_export_tests.rs`, `engine/tests/cases/provider_content.rs`, `engine/tests/providers_fake.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs` |
| `input_queued` | `instance_id` | `deferred`, `envelope_id`, `phase` | `core/src/v2/control.rs:2219`, `core/src/v2/control.rs:2239` | `tui/src/v2app.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `review/dogfood/crash.py`, `review/dogfood/queued_input.py` |
| `instance_created` | `id` | `instance_id` | `core/src/v2/control.rs:971` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `review/eval/r2-p6/anatomy.py` |
| `instance_interrupted` | `instance_id` | `phase`, `reason`, `request_id` | `core/src/v2/control.rs:4360` | observability only |
| `instance_lifecycle` | `instance` / `instance_id` | `lifecycle`, `reason` | `core/src/v2/control.rs:4203`, `core/src/v2/control.rs:4972` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/src/v2/diagnostics.rs`, `engine/src/v2/exec.rs`, `engine/tests/cli.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/dogfood/budget.py`, `review/dogfood/deadline.py`, `review/dogfood/lifecycle_run.py`, `review/dogfood/mcp_http.py` |
| `instance_profile_changed` | `instance_id` | `effort`, `model` | `core/src/v2/control.rs:1194` | observability only |
| `instance_reset` | `instance_id` | `closed`, `new_epoch`, `old_epoch`, `reason` | `core/src/v2/control.rs:888` | `engine/src/v2/exec.rs`, `review/dogfood/lifecycle_run.py` |
| `instance_spawned` | `instance_id` | `spawner`, `task` | `core/src/v2/control.rs:1032` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs` |
| `instance_terminated` | `instance_id` | `closed`, `grants_revoked`, `tasks_cancelled` | `core/src/v2/control.rs:4256` | observability only |
| `message_sent` | `recipient` | `correlation_id`, `envelope_id`, `sender` | `core/src/v2/control.rs:1127` | `review/dogfood/team_ring.py` |
| `operation_cancel_requested` | `operation_id` | `reason` | `core/src/v2/control.rs:3978` | observability only |
| `operation_cancelled` | `operation_id` | `reason` | `core/src/v2/control.rs:3973` | observability only |
| `operation_completed` | `operation_id` | `status` | `core/src/v2/control.rs:3697` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `review/benchmark/cadence.py` |
| `operation_dispatched` | `operation_id` | `operation_id` | `core/src/v2/control.rs:3943` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `review/benchmark/cadence.py` |
| `operation_stop_observed` | `operation` | — | `core/src/v2/control/work/takeover.rs:220` | observability only |
| `operation_unauthorized` | `operation_id` | `reason` | `core/src/v2/control.rs:935` | observability only |
| `request_began` | `instance_id` | `request_id` | `core/src/v2/control.rs:2575` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/benchmark/cadence.py` |
| `request_cancelled` | `&instance` / `instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:4297`, `core/src/v2/control.rs:4359` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs` |
| `request_failed` | `&instance` | `parked`, `reason`, `request_id` | `core/src/v2/control.rs:4146` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs` |
| `response_imported` | `&request_instance` | `decision_id`, `intents`, `phase`, `request_id` | `core/src/v2/control.rs:3645` | `tui/scripts/pty_graphics_smoke.py`, `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/v2_driver.rs` |
| `task_blocked` | `&task` | `operation_id`, `reason`, `task_id` | `core/src/v2/control.rs:3745` | `tui/src/v2app.rs`, `engine/src/v2/diagnostics/tests.rs`, `engine/src/v2/diagnostics.rs`, `review/dogfood/unknown_outcome.py` |
| `task_cancelled` | `&assignee` | `reason`, `task_id` | `core/src/v2/control.rs:1992` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/v2_supervisor.rs` |
| `task_completed` | `&requester` | `assignee`, `delivered`, `status`, `task_id` | `core/src/v2/control.rs:1939` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/dogfood/providers.py`, `review/eval/r2-p6/anatomy.py` |
| `task_delegated` | `assignee` | `capabilities`, `envelope_id`, `goal_id`, `requester`, `task_id` | `core/src/v2/control.rs:1602` | `tui/src/v2app.rs`, `review/dogfood/providers.py` |
| `task_dispatched` | `&assignee` / `assignee` | `envelope_id`, `granted`, `status`, `task_id` | `core/src/v2/control.rs:1484`, `core/src/v2/control.rs:1617` | observability only |
| `task_grants_revoked` | `task_id` | `revision`, `revoked`, `task_id` | `core/src/v2/control.rs:674` | observability only |
| `task_started` | `&assignee` | `task_id` | `core/src/v2/control.rs:1803` | `tui/src/v2app.rs`, `review/dogfood/providers.py` |
| `wait_satisfied` | `&instance_id` | `wait_id` | `core/src/v2/control.rs:3400` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `engine/tests/v2_driver.rs` |
| `work_admission_refused` | `instance` | `reason` | `core/src/v2/control/work.rs:27` | observability only |
| `work_attention` | `instance` | `reason`, `waits` | `core/src/v2/control/work.rs:841` | observability only |
| `work_blocked` | `instance` | `goal_id`, `reason` | `core/src/v2/control/work.rs:1065`, `core/src/v2/control/work.rs:432` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `engine/src/v2/diagnostics.rs`, `engine/tests/cases/autonomy.rs` |
| `work_checkpoint` | `instance` | `action`, `decision_id`, `goal_id` | `core/src/v2/control/work.rs:598` | `engine/tests/cases/autonomy.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/tool_catalogue.py` |
| `work_continuation` | `instance` | `goal_id`, `reason`, `repair_turns` | `core/src/v2/control/work.rs:436` | observability only |
| `work_input_admitted` | `instance` | `envelope_id`, `goal_id`, `policy_version` | `core/src/v2/control/work.rs:301` | `engine/src/v2/exec.rs` |
| `work_input_answered` | `instance` | `admission_id` | `core/src/v2/control/work/admission.rs:466` | observability only |
| `work_input_pending` | `instance` | `envelope_id` | `core/src/v2/control/work/admission.rs:32` | `engine/src/v2/exec.rs` |
| `work_policy_configured` | `leader` | `version` | `core/src/v2/control/work.rs:94` | observability only |
| `work_started` | `instance` | `billed_requests`, `goal_id`, `input_ref`, `intent_provenance` | `core/src/v2/control/work/admission.rs:295` | observability only |
| `workspace_takeover_blocked` | `operation` | — | `core/src/v2/control/work/takeover.rs:141` | observability only |

<!-- generated: end -->
