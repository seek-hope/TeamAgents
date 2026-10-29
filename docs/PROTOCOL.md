# The daemon protocol (`/v1`)

The session is a daemon behind one Unix socket (`<state root>/daemon.sock`), and everything else is a client of
it: the TUI, `teamagents exec`, the CLI verbs (`instances`, `tasks`, `approvals`, `authority`) and every probe
under `review/dogfood/`. DESIGN.md calls it "one JSON-lines protocol (`/v1`)" and states its guarantees
(handshake, `command_id` dedup, the event watermark); this document lists what it actually carries, generated from
the two dispatchers in the code. The [visual architecture](ARCHITECTURE.md) relates these methods to state,
transactions and recovery. Database schema 7, socket protocol 1 and work policy 1 are separate versions.

**Framing.** One JSON object per line, UTF-8, no other framing. The daemon speaks first: a greeting carrying
`server`, `protocol_version` (currently `1`), `session_id`, `state_root`, `permissions` and `workspace` — a client
that joins a running session learns the truth there instead of assuming the settings it asked for took effect
(D-41/D-57). Every request repeats `protocol_version`; a different version is refused.

**Requests.**

```json
{"protocol_version": 1, "request_id": "r-1", "method": "checkpoint", "params": {}}
{"protocol_version": 1, "request_id": "r-2", "command_id": "c-1", "method": "submit_input", "params": {"instance_id": "i-leader", "envelope_id": "e-1", "text": "…"}}
```

`request_id` is echoed in the reply. `command_id` is required for every mutating control command and is how
a client retries after a reconnect: the same id with the same payload returns the stored receipt, and the same
id with a different payload is refused (§6.3). Read methods take no `command_id`. Two daemon-only methods
also bypass the business-command receipt path: `input_paths` takes a `prefix` for workspace path completion;
`shutdown` returns `stopping` and `state_root` and stops this socket's daemon.

**Replies.** `{"request_id": …, "ok": true, "result": {…}}` or `{"request_id": …, "ok": false, "error": "…"}`. An
error is a human-readable sentence, not a code; the clients turn the ones they know into exit codes
(`docs/ACCEPTANCE.md`'s headless contract). A `result` is the handler's own JSON: keys not listed below are for
that method's caller and may grow, so a client should treat the shape it does not understand as opaque.

**Read methods** observe database facts; `previews` carries in-memory streaming text. Checkpoint adds
non-transactional process/configuration observations described below. Its database snapshot and watermark
share one read transaction; this is not a blanket guarantee for every read method. `checkpoint`
returns a snapshot plus its database watermark; `events` returns events since a watermark, catalogued in
[docs/EVENTS.md](EVENTS.md). The generated tables inventory directly read parameters and literal JSON fields
in the daemon and diagnostics helper. Helper-built projections, transitive parameters and defaults require
the linked source; this is not a complete machine-readable protocol schema. One page of the `inputs` read
method is bounded by both a row count (1..100) and a 4 MiB
payload-byte budget; the reply's `next_after` is the last included row's sequence, so a caller pages through
without losing or repeating an input (D-452). The read checks stored UTF-8 byte lengths before loading or
parsing payloads. If the first remaining payload alone exceeds 4 MiB, it returns an error naming its envelope
id and revision for explicit edit/cancel; it never silently skips, truncates or mutates that input.

`events` currently reads the watermark, then all events after `since`, without one encompassing read
transaction or a server-side page limit. Returned events may therefore exceed the sampled watermark. Clients
deduplicate by sequence. `resync_required` currently stays false; do not infer a completed reclaimed-cursor
recovery implementation from the field name.

**Work diagnostics (D-461).** Checkpoint instance rows add diagnosis; snapshot.tasks contains only
unsettled tasks. The tasks read still includes terminal tasks. Each task has diagnosis and assignee_activity:
task settlement never implies that the assignee's operation stopped. The shared diagnostics helper is
SELECT-only, in the caller's read transaction, with no events, model requests or scheduling side effects.

A diagnosis carries code, summary, attention, next_steps, optional reason (null if unavailable), waits and
operations. Wait rows preserve id, mode, conditions and timer_at; operation rows are for the instance's
current request/epoch and carry id, status and cancel_requested, not tool arguments or output. Codes include
idle_unsettled, idle, ready, waiting, wait_missing, waiting_approval, waiting_capability, model_pending,
tools_pending, checking, inactive, terminated, blocked, settled, missing_assignee and unknown. Clients must
accept unfamiliar codes and absent fields from older daemons. attention is an inspection hint, never a
permission or a deadlock verdict. A missing timer and elapsed time alone do not establish a stall.

**Environment and input feedback (D-479).** `snapshot.environment` is an observation outside the database
watermark: `workspace`, `execution` (host), `permissions` and `sources` describe the session at boot;
`configuration.files[]` reports each path as `unchanged`, `changed` or `unavailable`.
`restart_required` means a change was observed, and `comparison_available` is false if any comparison failed.
No file bytes or fingerprints are exposed. `leader_instruction_load` is null until the current Leader driver
assembles a prompt, then contains `epoch`, `observed_at`, `files[]` and `meaning`. Each file has `path`,
`status` (`loaded`/`unreadable`), and `bytes` or `reason`. It comes from the actual reads used for assembly but
is not a durable request receipt, a provider-consumption claim or a Skill-load audit. Older daemons may omit it.

`snapshot.input_delivery[]` is read in the checkpoint transaction: the latest 100 user-input envelopes for
the Leader, newest first, with `id` and `state` only. `ACCEPTED` is queued for a safe boundary; `DEFERRED` waits
for the current turn to end; `APPLIED` means delivered into context, never model/task completion;
`CANCELLED` and `SUPERSEDED` are terminal undelivered states. Clients must tolerate unknown states.
The TUI preserves its bounded receipt rows across checkpoint/history refreshes, including reconnect in the
same process; a newly launched TUI does not recreate all historical acknowledgements.

**Durable work (D-487).** `snapshot.work_policy_version = 1` identifies the enrolled scheduling contract.
Diagnosis adds `needs_input`, `work_blocked` and `stopped`, with a work projection containing scope, state,
checkpoint claims, recovery count and reason. Frozen request context, original input references and later
billing attribution remain separate. Ordinary clients submit input; they do not manage internal scheduling
reasons. `work_checkpoint` is a sole model control action, not a permission grant. Process-stop observations
are runtime-only and separate from immutable operation outcomes. Schema v7 prevents older writers from
ignoring these states; the socket protocol version remains 1.

**Commands** sent through the socket execute as `Identity::User` in one control transaction. Internal callers
may use other trusted identities, and some listed methods are runtime-only. `Identity passed` records whether
the dispatcher passes that identity argument to the handler; it does not itself establish an authorization
policy. Follow the handler and control wrapper for scope, revision, grant and budget checks.

<!-- generated: begin -->

### Read methods (answered from the session database)

| Method | Parameters | Reply keys | Answered at |
|---|---|---|---|
| `previews` | — | `previews` | `engine/src/v2/daemon.rs:701` |
| `checkpoint` | — | `snapshot`, `watermark` | `engine/src/v2/daemon.rs:707` |
| `events` | `since` | `events`, `resync_required`, `watermark` | `engine/src/v2/daemon.rs:714` |
| `history` | `instance_id`, `limit` | `entries`, `envelope_id`, `epoch`, `idx`, `instance_id`, `kind`, `message`, `work_goal_id`, `workspace` | `engine/src/v2/daemon.rs:723` |
| `surfaces` | `instance_id`, `limit` | `epoch`, `goal_id`, `instance_id`, `kind`, `offered_tools`, `request_id`, `status`, `surface_authorized`, `surfaces` | `engine/src/v2/daemon.rs:757` |
| `inputs` | — | `envelope_id`, `epoch`, `inputs`, `instance_id`, `next_after`, `payload`, `revision`, `sequence` | `engine/src/v2/daemon.rs:789` |
| `tasks` | — | `tasks` | `engine/src/v2/daemon.rs:840` |
| `approvals` | — | `approvals`, `id`, `operation_id`, `preview`, `tool` | `engine/src/v2/daemon.rs:849` |
| `goals` | — | `attached_instances`, `deadline`, `goals`, `id`, `known_usage`, `limits`, `status`, `summary`, `unknown_usage` | `engine/src/v2/daemon.rs:878` |
| `grants` | — | `action`, `capability_requests`, `delegable`, `grants`, `id`, `issuer`, `parent_grant_id`, `requester`, `resource_scope`, `revision`, `revoked`, `status`, `subject`, `task_id` | `engine/src/v2/daemon.rs:924` |

### Commands (executed by the control plane)

| Method | Parameters the handler reads | Identity passed | Handled at |
|---|---|---|---|
| `create_instance` | `id`, `profile`, `workspace_ref` | yes | `core/src/v2/control.rs:262` |
| `spawn_instance` | `acceptance_refs`, `capabilities`, `goal_id`, `instance_id`, `instructions`, `profile`, `task`, `task_id`, `workspace_ref` | yes | `core/src/v2/control.rs:263` |
| `issue_grant` | `action`, `delegable`, `parent_grant_id`, `resource_scope`, `subject`, `task_id` | yes | `core/src/v2/control.rs:264` |
| `revoke_grant` | `grant_id` | yes | `core/src/v2/control.rs:265` |
| `capability_requests` | — | no | `core/src/v2/control.rs:266` |
| `approve_capability` | `request_id` | yes | `core/src/v2/control.rs:267` |
| `deny_capability` | `request_id` | yes | `core/src/v2/control.rs:268` |
| `reauthorize_operation` | `operation_id` | no | `core/src/v2/control.rs:269` |
| `reset_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:270` |
| `configure_work` | `leader_id`, `limits` | yes | `core/src/v2/control.rs:271` |
| `create_goal` | `deadline`, `id`, `instance_id`, `limits`, `original_request_ref` | yes | `core/src/v2/control.rs:272` |
| `require_checks` | `checks`, `goal_id` | yes | `core/src/v2/control.rs:273` |
| `send_message` | `correlation_id`, `max_inbox`, `recipient`, `text` | yes | `core/src/v2/control.rs:274` |
| `drain_inbox` | `instance_id` | yes | `core/src/v2/control.rs:275` |
| `set_instance_profile` | `effort`, `instance_id`, `model` | yes | `core/src/v2/control.rs:276` |
| `delegate_task` | `acceptance_refs`, `assignee`, `dependencies`, `description`, `goal_id`, `task_id` | yes | `core/src/v2/control.rs:277` |
| `start_task` | `task_id` | yes | `core/src/v2/control.rs:278` |
| `complete_task` | `result_refs`, `status`, `summary`, `task_id` | yes | `core/src/v2/control.rs:279` |
| `cancel_task` | `reason`, `task_id` | yes | `core/src/v2/control.rs:280` |
| `read_history` | `epoch`, `instance_id`, `limit` | yes | `core/src/v2/control.rs:281` |
| `fire_timer` | `now` | yes | `core/src/v2/control.rs:282` |
| `blocked_report` | — | yes | `core/src/v2/control.rs:283` |
| `submit_input` | `after`, `checks`, `envelope_id`, `instance_id`, `text` | yes | `core/src/v2/control.rs:284` |
| `edit_input` | `envelope_id`, `instance_id`, `revision`, `text` | yes | `core/src/v2/control.rs:285` |
| `cancel_input` | `envelope_id`, `instance_id`, `revision`, `text` | yes | `core/src/v2/control.rs:286` |
| `begin_request` | `est_prompt_tokens`, `instance_id`, `offered_tools`, `prepared_work`, `request_id`, `request_ref`, `revision`, `surface_authorized` | yes | `core/src/v2/control.rs:287` |
| `record_attempt` | `attempt_id`, `elapsed_ms`, `error_class`, `request_id`, `response_ref`, `status`, `stream`, `unknown_usage`, `usage` | no | `core/src/v2/control.rs:288` |
| `begin_compression` | `est_prompt_tokens`, `instance_id`, `prepared_work`, `request_id`, `request_ref`, `revision` | yes | `core/src/v2/control.rs:289` |
| `compress_context` | `attempt_id`, `instance_id`, `keep_ids`, `request_id`, `summary` | yes | `core/src/v2/control.rs:290` |
| `fail_compression` | `reason`, `request_id` | no | `core/src/v2/control.rs:291` |
| `import_response` | `checkpoint`, `completion`, `decision_id`, `entry`, `grant_revision`, `intents`, `notes`, `request_id`, `wait` | yes | `core/src/v2/control.rs:292` |
| `observe_operation_stops` | `observations` | yes | `core/src/v2/control.rs:293` |
| `dispatch_operation` | `approval_required`, `operation_id`, `permission_revision` | no | `core/src/v2/control.rs:294` |
| `complete_operation` | `operation_id`, `receipt`, `status` | no | `core/src/v2/control.rs:295` |
| `cancel_operation` | `operation_id`, `reason` | no | `core/src/v2/control.rs:296` |
| `approve` | `approval_id`, `expires_at` | yes | `core/src/v2/control.rs:297` |
| `deny` | `approval_id` | yes | `core/src/v2/control.rs:298` |
| `review_decision` | `approval_id`, `outcome`, `rationale`, `reviewer_model`, `risk_level`, `user_authorization` | yes | `core/src/v2/control.rs:299` |
| `fail_request` | `park`, `reason`, `request_id` | no | `core/src/v2/control.rs:300` |
| `cancel_request` | `reason`, `request_id` | no | `core/src/v2/control.rs:301` |
| `interrupt_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:302` |
| `complete_goal` | `goal_id`, `instance_id` | no | `core/src/v2/control.rs:303` |
| `register_check_runs` | — | yes | `core/src/v2/control.rs:304` |
| `register_verification` | — | yes | `core/src/v2/control.rs:305` |
| `repair_completion` | `failures`, `goal_id`, `instance_id`, `round` | yes | `core/src/v2/control.rs:306` |
| `block_goal` | `goal_id`, `instance_id`, `reason` | yes | `core/src/v2/control.rs:307` |
| `deadline_warning` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:308` |
| `goal_checks_briefing` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:309` |
| `cancel_goal` | `goal_id` | yes | `core/src/v2/control.rs:310` |
| `close_completion` | `instance_id` | no | `core/src/v2/control.rs:311` |
| `artifact_abandon` | `id` | no | `core/src/v2/control.rs:312` |
| `set_lifecycle` | `instance_id`, `lifecycle`, `reason` | yes | `core/src/v2/control.rs:313` |
| `artifact_stage` | `digest`, `id`, `kind`, `owner_ref`, `owner_scope`, `size`, `storage_ref` | no | `core/src/v2/control.rs:314` |
| `artifact_publish` | `id` | no | `core/src/v2/control.rs:315` |
| `artifact_release_spent_owners` | `days`, `limit` | no | `core/src/v2/control.rs:316` |
| `artifact_gc_claim` | `limit` | no | `core/src/v2/control.rs:317` |
| `artifact_collect` | `id` | no | `core/src/v2/control.rs:318` |
| `prune_history` | `days`, `evidence`, `now` | no | `core/src/v2/control.rs:319` |
| `fork_reset` | `keep_instance`, `source_root`, `target_root` | yes | `core/src/v2/control.rs:320` |

### Object fields built by the daemon and diagnostics

This table inventories literal `json!({ … })` objects in `engine/src/v2/daemon.rs` and its
diagnostics helper. A row is one literal. Objects returned by other helpers, including durable
work projections, are outside this scan; consult their linked source contracts as well.

| Built at | Fields |
|---|---|
| `serve` | `id`, `name`, `root`, `active` |
| `serve` | `key`, `model`, `provider`, `efforts` |
| `mcp_rows` | `name`, `transport`, `required`, `oauth`, `credential` |
| `run_scheduler` | `id`, `instance_id` |
| `serve_client` | `server`, `protocol_version`, `session_id`, `state_root`, `permissions`, `workspace` |
| `serve_client` | `request_id`, `ok`, `error` |
| `handle` | `request_id`, `ok`, `result` |
| `handle` | `request_id`, `ok`, `error` |
| `handle` | — |
| `handle` | `workspace`, `execution`, `permissions`, `sources`, `leader_instruction_load`, `configuration` |
| `input_paths` arm | `stopping`, `state_root` |
| `read_snapshot` | `id`, `lifecycle`, `phase`, `model`, `reason`, `effort`, `can_run_shell`, `diagnosis` |
| `read_snapshot` | `status`, `known_usage`, `summary`, `unknown_usage`, `limits`, `deadline` |
| `read_snapshot` | `id`, `state` |
| `read_snapshot` | `instances`, `tasks`, `goal`, `input_delivery`, `work_policy_version`, `mcp`, `sessions`, `models` |
| `read_events` | `sequence`, `kind`, `scope`, `payload` |
| `apply_session_goal_limits` | — |
| `previews` arm | `previews` |
| `checkpoint` arm | `snapshot`, `watermark` |
| `events` arm | `events`, `watermark`, `resync_required` |
| `history` arm | `epoch`, `idx`, `kind`, `envelope_id`, `workspace`, `work_goal_id`, `message` |
| `history` arm | `instance_id`, `entries` |
| `surfaces` arm | `request_id`, `instance_id`, `epoch`, `goal_id`, `kind`, `status`, `offered_tools`, `surface_authorized` |
| `surfaces` arm | `surfaces` |
| `inputs` arm | `envelope_id`, `sequence`, `epoch`, `payload`, `revision` |
| `inputs` arm | `instance_id`, `inputs`, `next_after` |
| `tasks` arm | `tasks` |
| `approvals` arm | `id`, `operation_id`, `tool`, `preview` |
| `approvals` arm | `approvals` |
| `goals` arm | `id`, `summary`, `status`, `deadline`, `limits`, `known_usage`, `unknown_usage`, `attached_instances` |
| `goals` arm | — |
| `goals` arm | — |
| `goals` arm | `goals` |
| `grants` arm | `id`, `issuer`, `subject`, `action`, `resource_scope`, `parent_grant_id`, `delegable`, `task_id`, `revoked` |
| `grants` arm | `id`, `requester`, `subject`, `action`, `resource_scope`, `task_id`, `parent_grant_id`, `status` |
| `grants` arm | `grants`, `capability_requests`, `revision` |
| diagnostics: `diagnosis` | `code`, `summary`, `attention`, `next_steps`, `reason`, `waits`, `operations` |
| diagnostics: `read_sql` | `id`, `mode`, `conditions`, `timer_at` |
| diagnostics: `read_sql` | `id`, `status`, `cancel_requested` |
| diagnostics: `read_sql` | `id`, `goal_id`, `assignee`, `status`, `diagnosis`, `assignee_activity` |

<!-- generated: end -->
