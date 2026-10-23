# The daemon protocol (`/v1`)

The session is a daemon behind one Unix socket (`<state root>/daemon.sock`), and everything else is a client of
it: the TUI, `teamagents exec`, the CLI verbs (`instances`, `tasks`, `approvals`, `authority`) and every probe
under `review/dogfood/`. DESIGN.md calls it "one JSON-lines protocol (`/v1`)" and states its guarantees
(handshake, `command_id` dedup, the event watermark); this document lists what it actually carries, generated from
the two dispatchers in the code.

**Framing.** One JSON object per line, UTF-8, no other framing. The daemon speaks first: a greeting carrying
`server`, `protocol_version` (currently `1`), `session_id`, `state_root`, `permissions` and `workspace` — a client
that joins a running session learns the truth there instead of assuming the settings it asked for took effect
(D-41/D-57). Every request repeats `protocol_version`; a different version is refused.

**Requests.**

```json
{"protocol_version": 1, "request_id": "r-1", "method": "checkpoint", "params": {}}
{"protocol_version": 1, "request_id": "r-2", "command_id": "c-1", "method": "submit_input", "params": {"instance_id": "i-leader", "envelope_id": "e-1", "text": "…"}}
```

`request_id` is echoed in the reply. `command_id` is required for every *command* (everything that is not a read
method below) and is how a client retries after a reconnect: the same id with the same payload returns the stored
receipt, and the same id with a different payload is refused (§6.3). Read methods take no `command_id`.

**Replies.** `{"request_id": …, "ok": true, "result": {…}}` or `{"request_id": …, "ok": false, "error": "…"}`. An
error is a human-readable sentence, not a code; the clients turn the ones they know into exit codes
(`docs/ACCEPTANCE.md`'s headless contract). A `result` is the handler's own JSON: keys not listed below are for
that method's caller and may grow, so a client should treat the shape it does not understand as opaque.

**Read methods** are answered from the session database in one read transaction, without involving the
supervisor; they are the observation surface. `checkpoint` returns a snapshot plus the watermark it is consistent
with, and `events` returns what happened since a watermark — the reconnect contract, whose kinds are catalogued in
[docs/EVENTS.md](EVENTS.md). Every object a reply or the greeting carries — a snapshot's `instances[]` rows and
its `goal`, a task, an approval, a grant, a history entry, the envelopes themselves — has its fields in the
generated table at the end of this document, which is what the user guide's report contract (§1.2) points at
(D-173). One page of the `inputs` read method is bounded by both a row count (1..100) and a 4 MiB
payload-byte budget; the reply's `next_after` is the last included row's sequence, so a caller pages through
without losing or repeating an input (D-452). The read checks stored UTF-8 byte lengths before loading or
parsing payloads. If the first remaining payload alone exceeds 4 MiB, it returns an error naming its envelope
id and revision for explicit edit/cancel; it never silently skips, truncates or mutates that input.

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

**Commands** are executed by the control plane under the caller's identity (`Identity::User`) in one database
transaction, with the permission and budget checks of `core/src/v2/capability.rs`; `identity checked` below means
the handler re-checks who may do this rather than trusting the request.

<!-- generated: begin -->

### Read methods (answered from the session database)

| Method | Parameters | Reply keys | Answered at |
|---|---|---|---|
| `previews` | — | `previews` | `engine/src/v2/daemon.rs:670` |
| `checkpoint` | — | `snapshot`, `watermark` | `engine/src/v2/daemon.rs:676` |
| `events` | `since` | `events`, `resync_required`, `watermark` | `engine/src/v2/daemon.rs:683` |
| `history` | `instance_id`, `limit` | `entries`, `envelope_id`, `epoch`, `idx`, `instance_id`, `kind`, `message`, `workspace` | `engine/src/v2/daemon.rs:692` |
| `surfaces` | `instance_id`, `limit` | `epoch`, `goal_id`, `instance_id`, `kind`, `offered_tools`, `request_id`, `status`, `surface_authorized`, `surfaces` | `engine/src/v2/daemon.rs:725` |
| `inputs` | — | `envelope_id`, `epoch`, `inputs`, `instance_id`, `next_after`, `payload`, `revision`, `sequence` | `engine/src/v2/daemon.rs:757` |
| `tasks` | — | `tasks` | `engine/src/v2/daemon.rs:808` |
| `approvals` | — | `approvals`, `id`, `operation_id`, `preview`, `tool` | `engine/src/v2/daemon.rs:817` |
| `goals` | — | `attached_instances`, `deadline`, `goals`, `id`, `known_usage`, `limits`, `status`, `summary`, `unknown_usage` | `engine/src/v2/daemon.rs:846` |
| `grants` | — | `action`, `capability_requests`, `delegable`, `grants`, `id`, `issuer`, `parent_grant_id`, `requester`, `resource_scope`, `revision`, `revoked`, `status`, `subject`, `task_id` | `engine/src/v2/daemon.rs:892` |

### Commands (executed by the control plane)

| Method | Parameters the handler reads | Identity checked | Handled at |
|---|---|---|---|
| `create_instance` | `id`, `profile`, `workspace_ref` | yes | `core/src/v2/control.rs:245` |
| `spawn_instance` | `acceptance_refs`, `capabilities`, `goal_id`, `instance_id`, `instructions`, `profile`, `task`, `task_id`, `workspace_ref` | yes | `core/src/v2/control.rs:246` |
| `issue_grant` | `action`, `delegable`, `parent_grant_id`, `resource_scope`, `subject`, `task_id` | yes | `core/src/v2/control.rs:247` |
| `revoke_grant` | `grant_id` | yes | `core/src/v2/control.rs:248` |
| `capability_requests` | — | yes | `core/src/v2/control.rs:249` |
| `approve_capability` | `request_id` | yes | `core/src/v2/control.rs:250` |
| `deny_capability` | `request_id` | yes | `core/src/v2/control.rs:251` |
| `reauthorize_operation` | `operation_id` | yes | `core/src/v2/control.rs:252` |
| `reset_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:253` |
| `create_goal` | `deadline`, `id`, `instance_id`, `limits`, `original_request_ref` | yes | `core/src/v2/control.rs:254` |
| `require_checks` | `checks`, `goal_id` | yes | `core/src/v2/control.rs:255` |
| `send_message` | `correlation_id`, `max_inbox`, `recipient`, `text` | yes | `core/src/v2/control.rs:256` |
| `drain_inbox` | `instance_id` | yes | `core/src/v2/control.rs:257` |
| `set_instance_profile` | `effort`, `instance_id`, `model` | yes | `core/src/v2/control.rs:258` |
| `delegate_task` | `acceptance_refs`, `assignee`, `dependencies`, `description`, `goal_id`, `task_id` | yes | `core/src/v2/control.rs:259` |
| `start_task` | `task_id` | yes | `core/src/v2/control.rs:260` |
| `complete_task` | `result_refs`, `status`, `summary`, `task_id` | yes | `core/src/v2/control.rs:261` |
| `cancel_task` | `reason`, `task_id` | yes | `core/src/v2/control.rs:262` |
| `read_history` | `epoch`, `instance_id`, `limit` | yes | `core/src/v2/control.rs:263` |
| `fire_timer` | `now` | yes | `core/src/v2/control.rs:264` |
| `blocked_report` | — | yes | `core/src/v2/control.rs:265` |
| `submit_input` | `after`, `envelope_id`, `instance_id`, `text` | yes | `core/src/v2/control.rs:266` |
| `edit_input` | `envelope_id`, `instance_id`, `revision`, `text` | yes | `core/src/v2/control.rs:267` |
| `cancel_input` | `envelope_id`, `instance_id`, `revision`, `text` | yes | `core/src/v2/control.rs:268` |
| `begin_request` | `est_prompt_tokens`, `instance_id`, `offered_tools`, `request_id`, `request_ref`, `revision`, `surface_authorized` | yes | `core/src/v2/control.rs:269` |
| `record_attempt` | `attempt_id`, `elapsed_ms`, `error_class`, `request_id`, `response_ref`, `status`, `stream`, `unknown_usage`, `usage` | yes | `core/src/v2/control.rs:270` |
| `begin_compression` | `est_prompt_tokens`, `instance_id`, `request_id`, `request_ref` | yes | `core/src/v2/control.rs:271` |
| `compress_context` | `attempt_id`, `instance_id`, `keep_ids`, `request_id`, `summary` | yes | `core/src/v2/control.rs:272` |
| `fail_compression` | `reason`, `request_id` | yes | `core/src/v2/control.rs:273` |
| `import_response` | `completion`, `decision_id`, `entry`, `grant_revision`, `intents`, `notes`, `request_id`, `wait` | yes | `core/src/v2/control.rs:274` |
| `dispatch_operation` | `approval_required`, `operation_id`, `permission_revision` | no | `core/src/v2/control.rs:275` |
| `complete_operation` | `operation_id`, `receipt`, `status` | no | `core/src/v2/control.rs:276` |
| `cancel_operation` | `operation_id`, `reason` | yes | `core/src/v2/control.rs:277` |
| `approve` | `approval_id`, `expires_at` | yes | `core/src/v2/control.rs:278` |
| `deny` | `approval_id` | yes | `core/src/v2/control.rs:279` |
| `review_decision` | `approval_id`, `outcome`, `rationale`, `reviewer_model`, `risk_level`, `user_authorization` | yes | `core/src/v2/control.rs:280` |
| `fail_request` | `park`, `reason`, `request_id` | no | `core/src/v2/control.rs:281` |
| `cancel_request` | `reason`, `request_id` | yes | `core/src/v2/control.rs:282` |
| `interrupt_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:283` |
| `complete_goal` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:284` |
| `register_check_runs` | — | yes | `core/src/v2/control.rs:285` |
| `register_verification` | — | yes | `core/src/v2/control.rs:286` |
| `repair_completion` | `failures`, `goal_id`, `instance_id`, `round` | yes | `core/src/v2/control.rs:287` |
| `block_goal` | `goal_id`, `instance_id`, `reason` | yes | `core/src/v2/control.rs:288` |
| `deadline_warning` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:289` |
| `goal_checks_briefing` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:290` |
| `cancel_goal` | `goal_id` | yes | `core/src/v2/control.rs:291` |
| `close_completion` | `instance_id` | no | `core/src/v2/control.rs:292` |
| `artifact_abandon` | `id` | yes | `core/src/v2/control.rs:293` |
| `set_lifecycle` | `instance_id`, `lifecycle`, `reason` | yes | `core/src/v2/control.rs:294` |
| `artifact_stage` | `digest`, `id`, `kind`, `owner_ref`, `owner_scope`, `size`, `storage_ref` | no | `core/src/v2/control.rs:295` |
| `artifact_publish` | `id` | no | `core/src/v2/control.rs:296` |
| `artifact_release_spent_owners` | `days`, `limit` | no | `core/src/v2/control.rs:297` |
| `artifact_gc_claim` | `limit` | no | `core/src/v2/control.rs:298` |
| `artifact_collect` | `id` | no | `core/src/v2/control.rs:299` |
| `prune_history` | `days`, `evidence`, `now` | yes | `core/src/v2/control.rs:300` |
| `fork_reset` | `keep_instance`, `source_root`, `target_root` | yes | `core/src/v2/control.rs:301` |

### Every field the protocol carries in one object, and where it is built

The rows a client reads (`instances[]`, `tasks[]`, `approvals[]`, `grants[]`, `entries[]`), a
checkpoint's `goal` object and the greeting/reply envelopes are all built as `json!({ … })`
literals in `engine/src/v2/daemon.rs` and its diagnostics helper; this table is that list. A row is one literal.

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
| `input_paths` arm | `stopping`, `state_root` |
| `read_snapshot` | `id`, `lifecycle`, `phase`, `model`, `reason`, `effort`, `can_run_shell`, `diagnosis` |
| `read_snapshot` | `status`, `known_usage`, `summary`, `unknown_usage`, `limits`, `deadline` |
| `read_snapshot` | `instances`, `tasks`, `goal`, `mcp`, `sessions`, `models` |
| `read_events` | `sequence`, `kind`, `scope`, `payload` |
| `apply_session_goal_limits` | — |
| `previews` arm | `previews` |
| `checkpoint` arm | `snapshot`, `watermark` |
| `events` arm | `events`, `watermark`, `resync_required` |
| `history` arm | `epoch`, `idx`, `kind`, `envelope_id`, `workspace`, `message` |
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
