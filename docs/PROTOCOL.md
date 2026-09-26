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
[docs/EVENTS.md](EVENTS.md).

**Commands** are executed by the control plane under the caller's identity (`Identity::User`) in one database
transaction, with the permission and budget checks of `core/src/v2/capability.rs`; `identity checked` below means
the handler re-checks who may do this rather than trusting the request.

<!-- generated: begin -->

### Read methods (answered from the session database)

| Method | Parameters | Reply keys | Answered at |
|---|---|---|---|
| `checkpoint` | — | `snapshot`, `watermark` | `engine/src/v2/daemon.rs:273` |
| `events` | `since` | `events`, `resync_required`, `watermark` | `engine/src/v2/daemon.rs:280` |
| `history` | `instance_id`, `limit` | `entries`, `envelope_id`, `epoch`, `idx`, `instance_id`, `kind`, `message` | `engine/src/v2/daemon.rs:289` |
| `tasks` | — | `assignee`, `goal_id`, `id`, `status`, `tasks` | `engine/src/v2/daemon.rs:317` |
| `approvals` | — | `approvals`, `id`, `operation_id`, `preview`, `tool` | `engine/src/v2/daemon.rs:333` |
| `grants` | — | `action`, `grants`, `id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `revoked`, `subject` | `engine/src/v2/daemon.rs:364` |

### Commands (executed by the control plane)

| Method | Parameters the handler reads | Identity checked | Handled at |
|---|---|---|---|
| `create_instance` | `id`, `profile`, `workspace_ref` | yes | `core/src/v2/control.rs:119` |
| `spawn_instance` | `acceptance_refs`, `goal_id`, `instance_id`, `instructions`, `profile`, `task`, `task_id`, `workspace_ref` | yes | `core/src/v2/control.rs:120` |
| `issue_grant` | `action`, `parent_grant_id`, `resource_scope`, `subject` | yes | `core/src/v2/control.rs:121` |
| `revoke_grant` | `grant_id` | yes | `core/src/v2/control.rs:122` |
| `reauthorize_operation` | `operation_id` | yes | `core/src/v2/control.rs:123` |
| `reset_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:124` |
| `create_goal` | `deadline`, `id`, `instance_id`, `limits`, `original_request_ref` | yes | `core/src/v2/control.rs:125` |
| `send_message` | `correlation_id`, `max_inbox`, `recipient`, `text` | yes | `core/src/v2/control.rs:126` |
| `drain_inbox` | `instance_id` | yes | `core/src/v2/control.rs:127` |
| `delegate_task` | `acceptance_refs`, `assignee`, `dependencies`, `description`, `goal_id`, `task_id` | yes | `core/src/v2/control.rs:128` |
| `start_task` | `task_id` | yes | `core/src/v2/control.rs:129` |
| `complete_task` | `result_refs`, `status`, `summary`, `task_id` | yes | `core/src/v2/control.rs:130` |
| `cancel_task` | `reason`, `task_id` | yes | `core/src/v2/control.rs:131` |
| `read_history` | `epoch`, `instance_id`, `limit` | yes | `core/src/v2/control.rs:132` |
| `fire_timer` | `now` | yes | `core/src/v2/control.rs:133` |
| `blocked_report` | — | yes | `core/src/v2/control.rs:134` |
| `submit_input` | `envelope_id`, `instance_id`, `text` | yes | `core/src/v2/control.rs:135` |
| `begin_request` | `est_prompt_tokens`, `instance_id`, `request_id`, `request_ref`, `revision` | yes | `core/src/v2/control.rs:136` |
| `record_attempt` | `attempt_id`, `elapsed_ms`, `error_class`, `request_id`, `response_ref`, `status`, `unknown_usage`, `usage` | yes | `core/src/v2/control.rs:137` |
| `begin_compression` | `est_prompt_tokens`, `instance_id`, `request_id`, `request_ref` | yes | `core/src/v2/control.rs:138` |
| `compress_context` | `attempt_id`, `instance_id`, `keep_ids`, `request_id`, `summary` | yes | `core/src/v2/control.rs:139` |
| `fail_compression` | `reason`, `request_id` | yes | `core/src/v2/control.rs:140` |
| `import_response` | `completion`, `decision_id`, `entry`, `grant_revision`, `intents`, `notes`, `request_id`, `wait` | yes | `core/src/v2/control.rs:141` |
| `dispatch_operation` | `approval_required`, `operation_id`, `permission_revision` | no | `core/src/v2/control.rs:142` |
| `complete_operation` | `operation_id`, `receipt`, `status` | no | `core/src/v2/control.rs:143` |
| `cancel_operation` | `operation_id`, `reason` | yes | `core/src/v2/control.rs:144` |
| `approve` | `approval_id`, `expires_at` | yes | `core/src/v2/control.rs:145` |
| `deny` | `approval_id` | yes | `core/src/v2/control.rs:146` |
| `fail_request` | `park`, `reason`, `request_id` | no | `core/src/v2/control.rs:147` |
| `cancel_request` | `reason`, `request_id` | no | `core/src/v2/control.rs:148` |
| `complete_goal` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:149` |
| `register_check_runs` | `checks`, `goal_id`, `instance_id`, `round` | yes | `core/src/v2/control.rs:150` |
| `repair_completion` | `failures`, `goal_id`, `instance_id`, `round` | yes | `core/src/v2/control.rs:151` |
| `block_goal` | `goal_id`, `instance_id`, `reason` | yes | `core/src/v2/control.rs:152` |
| `close_completion` | `instance_id` | no | `core/src/v2/control.rs:153` |
| `artifact_abandon` | `id` | yes | `core/src/v2/control.rs:154` |
| `set_lifecycle` | `instance_id`, `lifecycle`, `reason` | yes | `core/src/v2/control.rs:155` |
| `artifact_stage` | `digest`, `id`, `kind`, `owner_ref`, `owner_scope`, `size`, `storage_ref` | no | `core/src/v2/control.rs:156` |
| `artifact_publish` | `id` | no | `core/src/v2/control.rs:157` |
| `artifact_gc_claim` | `limit` | no | `core/src/v2/control.rs:158` |

<!-- generated: end -->
