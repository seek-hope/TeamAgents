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

**The table is generated.** `python3 tools/event_catalogue.py --write` reads every `event(...)` call site in
`core/src` and rewrites the block below; `make hygiene` fails when the two drift (a kind emitted but not listed, a
listed kind that no longer exists, or a payload key that changed). The last column is a text search over `tui/`,
`engine/src`, `tools/` and `docs/`: **observability only** means nothing in this tree reads that kind by name —
the log is still a supported way to observe it (and the probes that do assert on a kind are listed).

<!-- generated: begin -->

| Event | Scope | Payload | Emitted at | Read by (outside core/src) |
|---|---|---|---|---|
| `approval_denied` | `approval_id` | `operation_id` | `core/src/v2/control.rs:5019` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `approval_granted` | `approval_id` | `approval_id`, `approver` | `core/src/v2/control.rs:4948`, `core/src/v2/control.rs:5255` | `tui/src/v2app.rs`, `engine/tests/v2_supervisor.rs` |
| `approval_requested` | `operation_id` | `approval_id` | `core/src/v2/control.rs:4790` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/update.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `approval_reviewed` | `approval_id` | `operation_id`, `outcome`, `review` | `core/src/v2/control.rs:5021`, `core/src/v2/control.rs:5262` | `engine/tests/v2_driver.rs`, `core/tests/v2_invariants.rs` |
| `artifact_abandoned` | `id` | `artifact_id` | `core/src/v2/control.rs:6340` | observability only |
| `artifact_collected` | `id` | `artifact_id` | `core/src/v2/control.rs:6756` | observability only |
| `artifact_owners_released` | `""` | `released` | `core/src/v2/control.rs:6501` | observability only |
| `artifacts_gc_claimed` | `""` | `claimed` | `core/src/v2/control.rs:6433` | `engine/tests/v2_daemon.rs` |
| `attempt_recorded` | `request_id` | `attempt_id`, `selected`, `status`, `stream`, `usage` | `core/src/v2/control.rs:3476` | `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `budget_refused` | `instance_id` | `est`, `known`, `max`, `request_id`, `reserved`, `work_id` | `core/src/v2/control.rs:3368` | `engine/src/v2/driver.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs` |
| `capability_approved` | `&subject` | `approver`, `grant_id`, `request_id`, `subject`, `task_id` | `core/src/v2/control.rs:2229` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `capability_denied` | `&task_id` | `decided_by`, `request_id`, `status`, `task_id` | `core/src/v2/control.rs:2297` | `tui/src/v2app.rs` |
| `capability_grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:948` | observability only |
| `capability_requested` | `subject` | `action`, `parent_grant_id`, `request_id`, `requester`, `resource_scope`, `subject`, `task_id` | `core/src/v2/control.rs:1785` | `tui/src/v2app.rs` |
| `check_round_registered` | `work_id` | `checks`, `round`, `work_id` | `core/src/v2/control.rs:5733` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs` |
| `completion_closed` | `instance_id` | `instance_id` | `core/src/v2/control.rs:6053` | `tui/src/v2app.rs`, `engine/tests/cases/rewind.rs`, `engine/tests/v2_driver.rs` |
| `completion_repair` | `instance_id` | `failures`, `round`, `work_id` | `core/src/v2/control.rs:5814` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `compression_began` | `instance_id` | `context_head`, `epoch`, `est_prompt_tokens`, `request_id`, `work_id` | `core/src/v2/control.rs:3551` | `engine/src/v2/exec/accounting.rs`, `engine/tests/v2_driver.rs` |
| `compression_failed` | `&instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:3706` | `engine/src/v2/exec/accounting.rs`, `engine/tests/v2_driver.rs` |
| `context_compressed` | `instance_id` | `covered`, `covers_to`, `kept`, `request_id`, `summary_id` | `core/src/v2/control.rs:3669` | `engine/src/v2/exec/accounting.rs`, `engine/tests/v2_driver.rs` |
| `conversation_rewound` | `&id` | `envelope`, `mode`, `new_epoch`, `old_epoch`, `position`, `restored` | `core/src/v2/control/rewind.rs:209` | `tui/src/v2app.rs` |
| `decision_consumed` | `&instance` | `decision_id` | `core/src/v2/control.rs:4639` | `tui/src/v2app.rs` |
| `envelopes_sealed` | `instance_id` | `envelope_ids`, `epoch`, `instance_id`, `reason` | `core/src/v2/control.rs:1178` | `engine/src/v2/exec.rs` |
| `fork_reset` | `keep` | `grants_revoked`, `instances_removed`, `kept` | `core/src/v2/control.rs:6596` | `engine/src/v2/sessions.rs` |
| `grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:885` | observability only |
| `grant_revoked` | `id` | `cascade`, `grant_id`, `revision` | `core/src/v2/control.rs:981` | observability only |
| `history_pruned` | `""` | `compacted`, `days`, `deliveries`, `events` | `core/src/v2/control.rs:6708` | observability only |
| `inbox_drained` | `instance_id` | `applied`, `sealed` | `core/src/v2/control.rs:1693` | `tui/src/v2app.rs`, `tui/tests/minimal.rs` |
| `input` | `instance_id` | `applied`, `context_head`, `envelope_id`, `epoch` | `core/src/v2/control.rs:1668`, `core/src/v2/control.rs:2864` | `tui/scripts/pty_v2_smoke.py`, `tui/src/queue.rs`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/providers/anthropic.rs`, `engine/src/providers/responses.rs`, `engine/src/v2/attachments.rs`, `engine/src/v2/diagnostics/tests.rs`, `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs`, `engine/src/v2/session_export_tests.rs`, `engine/tests/cases/mcp_policy.rs`, `engine/tests/cases/provider_content.rs`, `engine/tests/cases/rewind.rs`, `engine/tests/providers_fake.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs` |
| `input_queued` | `instance_id` | `after`, `deferred`, `envelope_id`, `phase` | `core/src/v2/control.rs:2821`, `core/src/v2/control.rs:2845` | `tui/src/v2app.rs`, `engine/src/v2/exec.rs`, `engine/tests/cases/event_pages.rs`, `engine/tests/v2_daemon.rs` |
| `instance_created` | `id` | `instance_id` | `core/src/v2/control.rs:1330` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs` |
| `instance_interrupted` | `instance_id` | `phase`, `reason`, `request_id` | `core/src/v2/control.rs:5524` | observability only |
| `instance_lifecycle` | `instance` / `instance_id` | `lifecycle`, `reason` | `core/src/v2/control.rs:5367`, `core/src/v2/control.rs:6140` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/src/v2/diagnostics.rs`, `engine/src/v2/exec.rs`, `engine/tests/cli.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs` |
| `instance_profile_changed` | `instance_id` | `effort`, `model` | `core/src/v2/control.rs:1583` | observability only |
| `instance_reset` | `instance_id` | `closed`, `new_epoch`, `old_epoch`, `reason` | `core/src/v2/control.rs:1220` | `tui/src/v2app.rs`, `engine/src/v2/exec.rs` |
| `instance_spawned` | `instance_id` | `spawner`, `task` | `core/src/v2/control.rs:1397` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs` |
| `instance_terminated` | `instance_id` | `closed`, `grants_revoked`, `tasks_cancelled` | `core/src/v2/control.rs:5420` | `engine/src/v2/exec.rs` |
| `manual_compaction_claimed` | `id` | `epoch`, `ready`, `revision` | `core/src/v2/control.rs:3167` | observability only |
| `manual_compaction_queued` | `id` | `epoch`, `revision` | `core/src/v2/control.rs:3144` | observability only |
| `mcp_policy_changed` | `"session"` | `revision`, `rule`, `service` | `core/src/v2/control/mcp_policy.rs:81` | observability only |
| `message_sent` | `recipient` | `correlation_id`, `envelope_id`, `sender` | `core/src/v2/control.rs:1498` | observability only |
| `operation_cancel_requested` | `operation_id` | `reason` | `core/src/v2/control.rs:4879` | observability only |
| `operation_cancelled` | `operation_id` | `reason` | `core/src/v2/control.rs:4874` | observability only |
| `operation_completed` | `operation_id` | `status` | `core/src/v2/control.rs:4502` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `tui/tests/team.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs` |
| `operation_dispatched` | `operation_id` | `operation_id` | `core/src/v2/control.rs:4844` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs` |
| `operation_stop_observed` | `operation` | — | `core/src/v2/control/work/takeover.rs:231` | observability only |
| `operation_unauthorized` | `operation_id` | `reason` | `core/src/v2/control.rs:1267` | observability only |
| `request_began` | `instance_id` | `context_head`, `epoch`, `request_id`, `work_id` | `core/src/v2/control.rs:3268` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/exec/accounting.rs`, `engine/tests/cases/event_pages.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs` |
| `request_cancelled` | `&instance` / `instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:5461`, `core/src/v2/control.rs:5523` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/exec/accounting.rs` |
| `request_failed` | `&instance` | `parked`, `reason`, `request_id` | `core/src/v2/control.rs:5310` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs` |
| `response_imported` | `&request_instance` | `decision_id`, `intents`, `phase`, `request_id` | `core/src/v2/control.rs:4437` | `tui/scripts/pty_graphics_smoke.py`, `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `tui/tests/reconnect.rs`, `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs`, `engine/tests/cases/catalog.rs`, `engine/tests/cases/supervisor_robustness.rs`, `engine/tests/v2_driver.rs` |
| `review_attempt_closed` | `&operation_id` | `operation_id`, `request_id`, `status`, `unknown_usage` | `core/src/v2/control.rs:5182` | `engine/src/v2/exec/accounting.rs`, `engine/tests/v2_driver.rs` |
| `review_began` | `&instance_id` | `est_prompt_tokens`, `operation_id`, `parent_request_id`, `request_id`, `work_id` | `core/src/v2/control.rs:5133` | `engine/src/v2/exec/accounting.rs` |
| `review_interrupted` | `&instance_id` | `operation_id`, `request_id`, `unknown_usage` | `core/src/v2/control.rs:5093` | `engine/src/v2/exec/accounting.rs` |
| `role_models_changed` | `&id` | — | `core/src/v2/control/role_models.rs:154` | observability only |
| `search_tree_update_failed` | `instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:3660` | `engine/tests/v2_driver.rs` |
| `search_tree_updated` | `&leader(tx)?` | `cause`, `cause_ref`, `node_count`, `revision`, `source_entry_id` | `core/src/v2/control/search_tree.rs:658` | `engine/tests/v2_driver.rs` |
| `task_blocked` | `&task` | `operation_id`, `reason`, `task_id` | `core/src/v2/control.rs:4550` | `tui/src/v2app.rs`, `engine/src/v2/diagnostics/tests.rs`, `engine/src/v2/diagnostics.rs` |
| `task_cancelled` | `&assignee` | `reason`, `task_id` | `core/src/v2/control.rs:2562` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/v2_supervisor.rs` |
| `task_completed` | `&requester` | `assignee`, `delivered`, `status`, `task_id` | `core/src/v2/control.rs:2484` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs` |
| `task_delegated` | `assignee` | `capabilities`, `envelope_id`, `requester`, `task_id`, `work_id` | `core/src/v2/control.rs:2101` | `tui/src/v2app.rs` |
| `task_dispatched` | `&assignee` / `assignee` | `envelope_id`, `granted`, `status`, `task_id` | `core/src/v2/control.rs:1983`, `core/src/v2/control.rs:2116` | observability only |
| `task_grants_revoked` | `task_id` | `revision`, `revoked`, `task_id` | `core/src/v2/control.rs:1006` | observability only |
| `task_started` | `&assignee` | `task_id` | `core/src/v2/control.rs:2332` | `tui/src/v2app.rs` |
| `wait_satisfied` | `&instance_id` | `wait_id` | `core/src/v2/control.rs:4146` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `engine/tests/v2_driver.rs` |
| `work_admission_refused` | `instance` | `reason` | `core/src/v2/control/work.rs:51` | observability only |
| `work_attention` | `instance` / `leader_id` | `reason`, `waits` | `core/src/v2/control.rs:732`, `core/src/v2/control/work.rs:890` | observability only |
| `work_blocked` | `instance` / `work_id` | `detached`, `reason`, `status`, `work_id` | `core/src/v2/control.rs:5882`, `core/src/v2/control/work.rs:1125`, `core/src/v2/control/work.rs:469` | `tui/src/v2app.rs`, `tui/tests/minimal.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/diagnostics.rs`, `engine/src/v2/exec.rs`, `engine/tests/cases/autonomy.rs`, `engine/tests/v2_driver.rs` |
| `work_cancelled` | `work_id` | `released`, `work_id` | `core/src/v2/control.rs:6150` | `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs` |
| `work_checkpoint` | `instance` | `action`, `decision_id`, `work_id` | `core/src/v2/control/work.rs:635` | `engine/src/v2/exec.rs`, `engine/tests/cases/autonomy.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `work_checks_briefed` | `work_id` | `checks` | `core/src/v2/control.rs:5962` | `engine/tests/v2_driver.rs` |
| `work_checks_required` | `work_id` | `added`, `checks`, `work_id` | `core/src/v2/control.rs:2732` | observability only |
| `work_completed` | `work_id` | `blocked_by`, `completion`, `detached`, `reason`, `status`, `work_id` | `core/src/v2/control.rs:5874`, `core/src/v2/control.rs:6310` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs`, `engine/tests/cases/event_pages.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs` |
| `work_continuation` | `&k.1` / `instance` | `reason`, `repair_turns`, `work_id` | `core/src/v2/control/work.rs:452`, `core/src/v2/control/work.rs:473`, `core/src/v2/control/work.rs:719` | observability only |
| `work_created` | `id` | `work_id` | `core/src/v2/control.rs:2661` | observability only |
| `work_deadline_refused` | `&instance_id` / `instance_id` | `kind`, `request_id`, `work_id` | `core/src/v2/control.rs:3221`, `core/src/v2/control.rs:3520`, `core/src/v2/control.rs:5112` | `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs` |
| `work_deadline_warning` | `work_id` | `seconds_left`, `work_id` | `core/src/v2/control.rs:6012` | `engine/tests/v2_driver.rs` |
| `work_input_admitted` | `instance` | `envelope_id`, `policy_version`, `work_id` | `core/src/v2/control/work.rs:328` | `engine/src/v2/exec/accounting.rs`, `engine/src/v2/exec.rs` |
| `work_input_answered` | `instance` | `admission_id` | `core/src/v2/control/work/admission.rs:469` | observability only |
| `work_input_pending` | `instance` | `envelope_id` | `core/src/v2/control/work/admission.rs:32` | `engine/src/v2/exec.rs` |
| `work_policy_configured` | `leader` | `version` | `core/src/v2/control/work.rs:118` | observability only |
| `work_started` | `instance` | `billed_requests`, `input_ref`, `intent_provenance`, `work_id` | `core/src/v2/control/work/admission.rs:298` | observability only |
| `workspace_takeover_blocked` | `operation` | — | `core/src/v2/control/work/takeover.rs:152` | observability only |

<!-- generated: end -->
