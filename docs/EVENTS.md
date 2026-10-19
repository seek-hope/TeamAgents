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
| `approval_denied` | `approval_id` | `operation_id` | `core/src/v2/control.rs:3760` | `tui/src/v2app.rs` |
| `approval_granted` | `approval_id` | `approval_id` | `core/src/v2/control.rs:3722`, `core/src/v2/control.rs:3816` | `tui/src/v2app.rs` |
| `approval_requested` | `operation_id` | `approval_id` | `core/src/v2/control.rs:3644` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/update.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs` |
| `approval_reviewed` | `approval_id` | `operation_id`, `outcome`, `review` | `core/src/v2/control.rs:3762`, `core/src/v2/control.rs:3817` | `engine/tests/v2_driver.rs` |
| `artifact_abandoned` | `id` | `artifact_id` | `core/src/v2/control.rs:4833` | observability only |
| `artifact_collected` | `id` | `artifact_id` | `core/src/v2/control.rs:5181` | observability only |
| `artifact_owners_released` | `""` | `released` | `core/src/v2/control.rs:4987` | observability only |
| `artifacts_gc_claimed` | `""` | `claimed` | `core/src/v2/control.rs:4919` | observability only |
| `attempt_recorded` | `request_id` | `attempt_id`, `selected`, `status`, `stream`, `usage` | `core/src/v2/control.rs:2559` | `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/benchmark/cadence.py` |
| `budget_refused` | `instance_id` | `est`, `goal_id`, `known`, `max`, `request_id`, `reserved` | `core/src/v2/control.rs:2454` | `engine/src/v2/driver.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/budget.py` |
| `capability_approved` | `&subject` | `grant_id`, `request_id`, `subject`, `task_id` | `core/src/v2/control.rs:1547` | observability only |
| `capability_denied` | `&task_id` | `request_id`, `status`, `task_id` | `core/src/v2/control.rs:1603` | observability only |
| `capability_grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:510` | observability only |
| `capability_requested` | `subject` | `action`, `parent_grant_id`, `request_id`, `requester`, `resource_scope`, `subject`, `task_id` | `core/src/v2/control.rs:1192` | observability only |
| `check_round_registered` | `goal_id` | `checks`, `goal_id`, `round` | `core/src/v2/control.rs:4275` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/maturity-2026-10-11/workflows.py` |
| `completion_closed` | `instance_id` | `instance_id` | `core/src/v2/control.rs:4596` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs` |
| `completion_repair` | `instance_id` | `failures`, `goal_id`, `round` | `core/src/v2/control.rs:4353` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/dogfood/two_gates.py`, `review/maturity-2026-10-11/workflows.py` |
| `compression_began` | `instance_id` | `est_prompt_tokens`, `goal_id`, `request_id` | `core/src/v2/control.rs:2615` | `engine/tests/v2_driver.rs` |
| `compression_failed` | `&instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:2736` | `engine/tests/v2_driver.rs` |
| `context_compressed` | `instance_id` | `covered`, `covers_to`, `kept`, `request_id`, `summary_id` | `core/src/v2/control.rs:2700` | `engine/tests/v2_driver.rs` |
| `decision_consumed` | `&instance` | `decision_id` | `core/src/v2/control.rs:3550` | `tui/src/v2app.rs` |
| `envelopes_sealed` | `instance_id` | `envelope_ids`, `epoch`, `instance_id`, `reason` | `core/src/v2/control.rs:740` | `engine/src/v2/exec.rs` |
| `fork_reset` | `keep` | `grants_revoked`, `instances_removed`, `kept` | `core/src/v2/control.rs:5071` | `engine/src/v2/sessions.rs` |
| `goal_blocked` | `goal_id` | `detached`, `goal_id`, `reason`, `status` | `core/src/v2/control.rs:4425` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/two_gates.py` |
| `goal_cancelled` | `goal_id` | `goal_id`, `released` | `core/src/v2/control.rs:4693` | observability only |
| `goal_checks_briefed` | `goal_id` | `checks` | `core/src/v2/control.rs:4505` | `engine/tests/v2_driver.rs` |
| `goal_checks_required` | `goal_id` | `added`, `checks`, `goal_id` | `core/src/v2/control.rs:1988` | observability only |
| `goal_completed` | `goal_id` | `blocked_by`, `completion`, `detached`, `goal_id`, `reason`, `status` | `core/src/v2/control.rs:4417`, `core/src/v2/control.rs:4803` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/tests/v2_daemon.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs`, `review/benchmark/tb_silence.py`, `review/dogfood/checks.py`, `review/dogfood/stale_check.py`, `review/dogfood/two_gates.py`, `review/dogfood/unknown_outcome.py` |
| `goal_created` | `id` | `goal_id` | `core/src/v2/control.rs:1917` | observability only |
| `goal_deadline_refused` | `instance_id` | `goal_id`, `kind`, `request_id` | `core/src/v2/control.rs:2330`, `core/src/v2/control.rs:2590` | `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs`, `review/dogfood/deadline.py` |
| `goal_deadline_warning` | `goal_id` | `goal_id`, `seconds_left` | `core/src/v2/control.rs:4555` | `engine/tests/v2_driver.rs`, `review/benchmark/deadline_wrapup_ab.py` |
| `grant_issued` | `subject` | `action`, `delegable`, `grant_id`, `parent_grant_id`, `resource_scope`, `revision`, `subject`, `task_id` | `core/src/v2/control.rs:453` | observability only |
| `grant_revoked` | `id` | `cascade`, `grant_id`, `revision` | `core/src/v2/control.rs:543` | observability only |
| `history_pruned` | `""` | `days`, `deliveries`, `events` | `core/src/v2/control.rs:5141` | observability only |
| `inbox_drained` | `instance_id` | `applied`, `sealed` | `core/src/v2/control.rs:1109` | observability only |
| `input` | `instance_id` | `applied`, `envelope_id` | `core/src/v2/control.rs:2080` | `tui/scripts/pty_v2_smoke.py`, `tui/src/queue.rs`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/providers/anthropic.rs`, `engine/src/providers/responses.rs`, `engine/src/v2/attachments.rs`, `engine/tests/cases/provider_content.rs`, `engine/tests/providers_fake.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `engine/tests/v2_supervisor.rs` |
| `input_queued` | `instance_id` | `envelope_id`, `phase` | `core/src/v2/control.rs:2041` | `engine/src/v2/exec.rs`, `engine/tests/v2_daemon.rs`, `review/dogfood/crash.py`, `review/dogfood/queued_input.py` |
| `instance_created` | `id` | `instance_id` | `core/src/v2/control.rs:864` | `tui/src/v2app.rs`, `review/eval/r2-p6/anatomy.py` |
| `instance_interrupted` | `instance_id` | `phase`, `reason`, `request_id` | `core/src/v2/control.rs:4067` | observability only |
| `instance_lifecycle` | `instance` / `instance_id` | `lifecycle`, `reason` | `core/src/v2/control.rs:3912`, `core/src/v2/control.rs:4683` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/daemon.rs`, `engine/src/v2/exec.rs`, `engine/tests/cli.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/dogfood/budget.py`, `review/dogfood/deadline.py`, `review/dogfood/lifecycle_run.py`, `review/dogfood/mcp_http.py` |
| `instance_reset` | `instance_id` | `closed`, `new_epoch`, `old_epoch`, `reason` | `core/src/v2/control.rs:781` | `engine/src/v2/exec.rs`, `review/dogfood/lifecycle_run.py` |
| `instance_spawned` | `instance_id` | `spawner`, `task` | `core/src/v2/control.rs:925` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs` |
| `instance_terminated` | `instance_id` | `closed`, `grants_revoked`, `tasks_cancelled` | `core/src/v2/control.rs:3965` | observability only |
| `message_sent` | `recipient` | `correlation_id`, `envelope_id`, `sender` | `core/src/v2/control.rs:1020` | `review/dogfood/team_ring.py` |
| `operation_cancel_requested` | `operation_id` | `reason` | `core/src/v2/control.rs:3697` | observability only |
| `operation_cancelled` | `operation_id` | `reason` | `core/src/v2/control.rs:3692` | observability only |
| `operation_completed` | `operation_id` | `status` | `core/src/v2/control.rs:3446` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs`, `review/benchmark/cadence.py` |
| `operation_dispatched` | `operation_id` | `operation_id` | `core/src/v2/control.rs:3662` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_mcp.rs`, `review/benchmark/cadence.py` |
| `operation_unauthorized` | `operation_id` | `reason` | `core/src/v2/control.rs:828` | observability only |
| `request_began` | `instance_id` | `request_id` | `core/src/v2/control.rs:2361` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/benchmark/cadence.py` |
| `request_cancelled` | `&instance` / `instance_id` | `reason`, `request_id` | `core/src/v2/control.rs:4005`, `core/src/v2/control.rs:4066` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs` |
| `request_failed` | `&instance` | `parked`, `reason`, `request_id` | `core/src/v2/control.rs:3861` | `tui/src/v2app.rs`, `tui/tests/v2app_tests.rs`, `engine/src/v2/exec.rs`, `engine/tests/v2_driver.rs` |
| `response_imported` | `&request_instance` | `decision_id`, `intents`, `phase`, `request_id` | `core/src/v2/control.rs:3394` | `tui/scripts/pty_graphics_smoke.py`, `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/v2_driver.rs` |
| `task_blocked` | `&task` | `operation_id`, `reason`, `task_id` | `core/src/v2/control.rs:3482` | `tui/src/v2app.rs`, `review/dogfood/unknown_outcome.py` |
| `task_cancelled` | `&assignee` | `reason`, `task_id` | `core/src/v2/control.rs:1818` | `tui/scripts/pty_v2_smoke.py`, `tui/src/v2app.rs`, `engine/tests/v2_supervisor.rs` |
| `task_completed` | `&requester` | `assignee`, `delivered`, `status`, `task_id` | `core/src/v2/control.rs:1768` | `tui/src/v2app.rs`, `engine/tests/v2_driver.rs`, `engine/tests/v2_supervisor.rs`, `review/dogfood/providers.py`, `review/eval/r2-p6/anatomy.py` |
| `task_delegated` | `assignee` | `capabilities`, `envelope_id`, `goal_id`, `requester`, `task_id` | `core/src/v2/control.rs:1437` | `tui/src/v2app.rs`, `review/dogfood/providers.py` |
| `task_dispatched` | `&assignee` / `assignee` | `envelope_id`, `granted`, `status`, `task_id` | `core/src/v2/control.rs:1319`, `core/src/v2/control.rs:1452` | observability only |
| `task_grants_revoked` | `task_id` | `revision`, `revoked`, `task_id` | `core/src/v2/control.rs:568` | observability only |
| `task_started` | `&assignee` | `task_id` | `core/src/v2/control.rs:1637` | `tui/src/v2app.rs`, `review/dogfood/providers.py` |
| `wait_satisfied` | `&instance_id` | `wait_id` | `core/src/v2/control.rs:3165` | `engine/tests/v2_driver.rs` |

<!-- generated: end -->
