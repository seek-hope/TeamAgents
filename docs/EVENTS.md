# The event log: every kind this build emits

Every control command appends to one per-session event log; the daemon answers `events` from a watermark, so a
client that reconnects reads what it missed instead of guessing (`docs/DESIGN.md` §? — "a reconnect first reads
state and its event watermark in one read snapshot and then reads events"), and `teamagents exec --json` builds
its report from the same stream (D-49/D-72: a run reports *its own* outcome, which it identifies by the events
that follow its input).

Each row carries the session, a per-session increasing sequence, a kind, a scope (usually the instance, task,
work or operation it is about) and a JSON payload. Two things it is not: the log is **not** the state — the
session tables are authoritative and are what a reader should trust for "what is true now" — and it is **not**
transactional for the outside world: a receipt is what judges a tool call, and a service the command left behind
belongs to the user (D-41).

**Families.** `work_*` and `check_*`/`completion_*` are the work's life at the completion boundary; `request_*`,
`attempt_recorded` and `response_imported` are the model calls inside a turn; `task_*` and `envelope*`/`message_sent`
are delegation and messages; `operation_*` and `decision_consumed` are tool dispatch; `approval_*` and `grant_*`
are the permission surfaces; `instance_*` is lifecycle (create, spawn, reset, terminate); `context_compressed`,
`compression_*` and `inbox_drained` are context delivery; `artifact*` is staging and GC; `budget_refused` and
`work_deadline_refused` are the admission gates.

**The table is generated.** `python3 review/event_catalogue.py --write` reads every `event(...)` call site in
`core/src` and rewrites the block below; `make hygiene` fails when the two drift (a kind emitted but not listed, a
listed kind that no longer exists, or a payload key that changed). The last column is a text search over `tui/`,
`engine/src`, `review/` and `docs/`: **observability only** means nothing in this tree reads that kind by name —
the log is still a supported way to observe it (and the probes that do assert on a kind are listed).

<!-- generated: begin -->

| Event | Scope | Payload | Emitted at | Read by (outside core/src) |
|---|---|---|---|---|
| `approval_denied` | `approval_id` | `operation_id` | `core/src/v2/control.rs:4087` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `approval_granted` | `approval_id` | `approval_id` | `core/src/v2/control.rs:4049`, `core/src/v2/control.rs:4322` | `tui/src/v2app.rs` |
| `approval_requested` | `operation_id` | `approval_id` | `core/src/v2/control.rs:3971` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/update.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `approval_reviewed` | `approval_id` | `operation_id`, `outcome`, `review` | `core/src/v2/control.rs:4089`, `core/src/v2/control.rs:4323` | `engine/tests/v2_driver.rs` |
| `artifact_abandoned` | `id` | `artifact_id` | `core/src/v2/control.rs:5378` | observability only |
| `artifact_collected` | `id` | `artifact_id` | `core/src/v2/control.rs:5779` | observability only |
| `artifact_owners_released` | `""` | `released` | `core/src/v2/control.rs:5532` | observability only |
| `artifacts_gc_claimed` | `""` | `claimed` | `core/src/v2/control.rs:5464` | `engine/tests/v2_daemon.rs` |
| `attempt_recorded` | `request_id` | `attempt_id`, `selected`, `status`, `stream`, `usage` | `core/src/v2/control.rs:2802` | `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/benchmark/cadence.py` |
| `budget_refused` | `instance_id` | `est`, `known`, `max`, `request_id`, `reserved`, `work_id` | `core/src/v2/control.rs:2694` | `engine/src/v2/driver.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/budget.py` |
| `capability_approved` | `&subject` | `grant_id`, `request_id`, `subject`, `task_id` | `core/src/v2/control.rs:1737` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `capability_denied` | `&task_id` | `request_id`, `status`, `task_id` | `core/src/v2/control.rs:1793` | `tui/src/v2app.rs` |
| `capability_grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:636` | observability only |
| `capability_requested` | `subject` | `action`, `parent_grant_id`, `request_id`, `requester`, `resource_scope`, `subject`, `task_id` | `core/src/v2/control.rs:1382` | `tui/src/v2app.rs` |
| `check_round_registered` | `work_id` | `checks`, `round`, `work_id` | `core/src/v2/control.rs:4789` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/maturity-2026-10-11/workflows.py` |
| `completion_closed` | `instance_id` | `instance_id` | `core/src/v2/control.rs:5110` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs` |
| `completion_repair` | `instance_id` | `failures`, `round`, `work_id` | `core/src/v2/control.rs:4867` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/dogfood/two_gates.py`, `review/maturity-2026-10-11/workflows.py` |
| `compression_began` | `instance_id` | `est_prompt_tokens`, `request_id`, `work_id` | `core/src/v2/control.rs:2870` | `engine/tests/v2_driver.rs` |
| `compression_failed` | `&instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:3011` | `engine/tests/v2_driver.rs` |
| `context_compressed` | `instance_id` | `covered`, `covers_to`, `kept`, `request_id`, `summary_id` | `core/src/v2/control.rs:2974` | `engine/tests/v2_driver.rs` |
| `decision_consumed` | `&instance` | `decision_id` | `core/src/v2/control.rs:3873` | `tui/src/v2app.rs` |
| `envelopes_sealed` | `instance_id` | `envelope_ids`, `epoch`, `instance_id`, `reason` | `core/src/v2/control.rs:866` | `engine/src/v2/exec.rs` |
| `fork_reset` | `keep` | `grants_revoked`, `instances_removed`, `kept` | `core/src/v2/control.rs:5621` | `engine/src/v2/sessions.rs` |
| `grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:579` | observability only |
| `grant_revoked` | `id` | `cascade`, `grant_id`, `revision` | `core/src/v2/control.rs:669` | observability only |
| `history_pruned` | `""` | `compacted`, `days`, `deliveries`, `events` | `core/src/v2/control.rs:5733` | observability only |
| `inbox_drained` | `instance_id` | `applied`, `sealed` | `core/src/v2/control.rs:1299` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `input` | `instance_id` | `applied`, `envelope_id` | `core/src/v2/control.rs:2282` | `tui/scripts/pty_v2_smoke.py`, `tui/src/queue.rs`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/providers/anthropic.rs`, `engine/src/providers/responses.rs`, `engine/src/v2/attachments.rs`, `engine/src/v2/diagnostics/tests.rs`, `engine/src/v2/exec.rs`, `engine/src/v2/session_export_tests.rs`, `engine/tests/cases/provider_content.rs`, `engine/tests/providers_fake.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs` |
| `input_queued` | `instance_id` | `deferred`, `envelope_id`, `phase` | `core/src/v2/control.rs:2245`, `core/src/v2/control.rs:2265` | `tui/src/v2app.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `review/dogfood/crash.py`, `review/dogfood/queued_input.py` |
| `instance_created` | `id` | `instance_id` | `core/src/v2/control.rs:996` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `review/eval/r2-p6/anatomy.py` |
| `instance_interrupted` | `instance_id` | `phase`, `reason`, `request_id` | `core/src/v2/control.rs:4585` | observability only |
| `instance_lifecycle` | `instance` / `instance_id` | `lifecycle`, `reason` | `core/src/v2/control.rs:4428`, `core/src/v2/control.rs:5197` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/src/v2/diagnostics.rs`, `engine/src/v2/exec.rs`, `engine/tests/cli.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/dogfood/budget.py`, `review/dogfood/deadline.py`, `review/dogfood/lifecycle_run.py`, `review/dogfood/mcp_http.py` |
| `instance_profile_changed` | `instance_id` | `effort`, `model` | `core/src/v2/control.rs:1219` | observability only |
| `instance_reset` | `instance_id` | `closed`, `new_epoch`, `old_epoch`, `reason` | `core/src/v2/control.rs:908` | `engine/src/v2/exec.rs`, `review/dogfood/lifecycle_run.py` |
| `instance_spawned` | `instance_id` | `spawner`, `task` | `core/src/v2/control.rs:1057` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs` |
| `instance_terminated` | `instance_id` | `closed`, `grants_revoked`, `tasks_cancelled` | `core/src/v2/control.rs:4481` | observability only |
| `message_sent` | `recipient` | `correlation_id`, `envelope_id`, `sender` | `core/src/v2/control.rs:1152` | `review/dogfood/team_ring.py` |
| `operation_cancel_requested` | `operation_id` | `reason` | `core/src/v2/control.rs:4024` | observability only |
| `operation_cancelled` | `operation_id` | `reason` | `core/src/v2/control.rs:4019` | observability only |
| `operation_completed` | `operation_id` | `status` | `core/src/v2/control.rs:3743` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `review/benchmark/cadence.py` |
| `operation_dispatched` | `operation_id` | `operation_id` | `core/src/v2/control.rs:3989` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `review/benchmark/cadence.py` |
| `operation_stop_observed` | `operation` | — | `core/src/v2/control/work/takeover.rs:230` | observability only |
| `operation_unauthorized` | `operation_id` | `reason` | `core/src/v2/control.rs:955` | observability only |
| `request_began` | `instance_id` | `request_id` | `core/src/v2/control.rs:2601` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/benchmark/cadence.py` |
| `request_cancelled` | `&instance` / `instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:4522`, `core/src/v2/control.rs:4584` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs` |
| `request_failed` | `&instance` | `parked`, `reason`, `request_id` | `core/src/v2/control.rs:4371` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs` |
| `response_imported` | `&request_instance` | `decision_id`, `intents`, `phase`, `request_id` | `core/src/v2/control.rs:3691` | `tui/scripts/pty_graphics_smoke.py`, `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/cases/supervisor_robustness.rs`, `engine/tests/v2_driver.rs` |
| `review_attempt_closed` | `&operation_id` | `operation_id`, `request_id`, `status`, `unknown_usage` | `core/src/v2/control.rs:4249` | `engine/tests/v2_driver.rs` |
| `review_began` | `&instance_id` | `est_prompt_tokens`, `operation_id`, `request_id`, `work_id` | `core/src/v2/control.rs:4200` | observability only |
| `review_interrupted` | `&instance_id` | `operation_id`, `request_id`, `unknown_usage` | `core/src/v2/control.rs:4160` | observability only |
| `search_tree_update_failed` | `instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:2965` | `engine/tests/v2_driver.rs` |
| `search_tree_updated` | `&leader(tx)?` | `cause`, `cause_ref`, `node_count`, `revision`, `source_entry_id` | `core/src/v2/control/search_tree.rs:658` | `engine/tests/v2_driver.rs` |
| `task_blocked` | `&task` | `operation_id`, `reason`, `task_id` | `core/src/v2/control.rs:3791` | `tui/src/v2app.rs`, `engine/src/v2/diagnostics/tests.rs`, `engine/src/v2/diagnostics.rs`, `review/dogfood/unknown_outcome.py` |
| `task_cancelled` | `&assignee` | `reason`, `task_id` | `core/src/v2/control.rs:2017` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/v2_supervisor.rs` |
| `task_completed` | `&requester` | `assignee`, `delivered`, `status`, `task_id` | `core/src/v2/control.rs:1964` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/dogfood/providers.py`, `review/eval/r2-p6/anatomy.py` |
| `task_delegated` | `assignee` | `capabilities`, `envelope_id`, `requester`, `task_id`, `work_id` | `core/src/v2/control.rs:1627` | `tui/src/v2app.rs`, `review/dogfood/providers.py` |
| `task_dispatched` | `&assignee` / `assignee` | `envelope_id`, `granted`, `status`, `task_id` | `core/src/v2/control.rs:1509`, `core/src/v2/control.rs:1642` | observability only |
| `task_grants_revoked` | `task_id` | `revision`, `revoked`, `task_id` | `core/src/v2/control.rs:694` | observability only |
| `task_started` | `&assignee` | `task_id` | `core/src/v2/control.rs:1828` | `tui/src/v2app.rs`, `review/dogfood/providers.py` |
| `wait_satisfied` | `&instance_id` | `wait_id` | `core/src/v2/control.rs:3446` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `engine/tests/v2_driver.rs` |
| `work_admission_refused` | `instance` | `reason` | `core/src/v2/control/work.rs:27` | observability only |
| `work_attention` | `instance` | `reason`, `waits` | `core/src/v2/control/work.rs:843` | observability only |
| `work_blocked` | `instance` / `work_id` | `detached`, `reason`, `status`, `work_id` | `core/src/v2/control.rs:4939`, `core/src/v2/control/work.rs:1067`, `core/src/v2/control/work.rs:434` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/diagnostics.rs`, `engine/tests/cases/autonomy.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/two_gates.py` |
| `work_cancelled` | `work_id` | `released`, `work_id` | `core/src/v2/control.rs:5207` | observability only |
| `work_checkpoint` | `instance` | `action`, `decision_id`, `work_id` | `core/src/v2/control/work.rs:600` | `engine/tests/cases/autonomy.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/tool_catalogue.py` |
| `work_checks_briefed` | `work_id` | `checks` | `core/src/v2/control.rs:5019` | `engine/tests/v2_driver.rs` |
| `work_checks_required` | `work_id` | `added`, `checks`, `work_id` | `core/src/v2/control.rs:2187` | observability only |
| `work_completed` | `work_id` | `blocked_by`, `completion`, `detached`, `reason`, `status`, `work_id` | `core/src/v2/control.rs:4931`, `core/src/v2/control.rs:5348` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/tests/cases/autonomy.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs`, `review/benchmark/tb_silence.py`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/dogfood/two_gates.py`, `review/dogfood/unknown_outcome.py` |
| `work_continuation` | `instance` | `reason`, `repair_turns`, `work_id` | `core/src/v2/control/work.rs:438` | observability only |
| `work_created` | `id` | `work_id` | `core/src/v2/control.rs:2116` | observability only |
| `work_deadline_refused` | `&instance_id` / `instance_id` | `kind`, `request_id`, `work_id` | `core/src/v2/control.rs:2564`, `core/src/v2/control.rs:2839`, `core/src/v2/control.rs:4179` | `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/deadline.py` |
| `work_deadline_warning` | `work_id` | `seconds_left`, `work_id` | `core/src/v2/control.rs:5069` | `engine/tests/v2_driver.rs`, `review/benchmark/deadline_wrapup_ab.py` |
| `work_input_admitted` | `instance` | `envelope_id`, `policy_version`, `work_id` | `core/src/v2/control/work.rs:303` | `engine/src/v2/exec.rs` |
| `work_input_answered` | `instance` | `admission_id` | `core/src/v2/control/work/admission.rs:469` | observability only |
| `work_input_pending` | `instance` | `envelope_id` | `core/src/v2/control/work/admission.rs:32` | `engine/src/v2/exec.rs` |
| `work_policy_configured` | `leader` | `version` | `core/src/v2/control/work.rs:94` | observability only |
| `work_started` | `instance` | `billed_requests`, `input_ref`, `intent_provenance`, `work_id` | `core/src/v2/control/work/admission.rs:298` | observability only |
| `workspace_takeover_blocked` | `operation` | — | `core/src/v2/control/work/takeover.rs:151` | observability only |

<!-- generated: end -->
