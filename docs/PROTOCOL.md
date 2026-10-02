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
(D-173).

**Commands** are executed by the control plane under the caller's identity (`Identity::User`) in one database
transaction, with the permission and budget checks of `core/src/v2/capability.rs`; `identity checked` below means
the handler re-checks who may do this rather than trusting the request.

<!-- generated: begin -->

### Read methods (answered from the session database)

| Method | Parameters | Reply keys | Answered at |
|---|---|---|---|
| `previews` | — | `previews` | `engine/src/v2/daemon.rs:478` |
| `checkpoint` | — | `snapshot`, `watermark` | `engine/src/v2/daemon.rs:484` |
| `events` | `since` | `events`, `resync_required`, `watermark` | `engine/src/v2/daemon.rs:491` |
| `history` | `instance_id`, `limit` | `entries`, `envelope_id`, `epoch`, `idx`, `instance_id`, `kind`, `message` | `engine/src/v2/daemon.rs:500` |
| `surfaces` | `instance_id`, `limit` | `epoch`, `goal_id`, `instance_id`, `kind`, `offered_tools`, `request_id`, `status`, `surface_authorized`, `surfaces` | `engine/src/v2/daemon.rs:531` |
| `tasks` | — | `assignee`, `goal_id`, `id`, `status`, `tasks` | `engine/src/v2/daemon.rs:563` |
| `approvals` | — | `approvals`, `id`, `operation_id`, `preview`, `tool` | `engine/src/v2/daemon.rs:579` |
| `goals` | — | `attached_instances`, `deadline`, `goals`, `id`, `known_usage`, `limits`, `status`, `unknown_usage` | `engine/src/v2/daemon.rs:608` |
| `grants` | — | `action`, `grants`, `id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `revoked`, `subject` | `engine/src/v2/daemon.rs:643` |

### Commands (executed by the control plane)

| Method | Parameters the handler reads | Identity checked | Handled at |
|---|---|---|---|
| `create_instance` | `id`, `profile`, `workspace_ref` | yes | `core/src/v2/control.rs:126` |
| `spawn_instance` | `acceptance_refs`, `goal_id`, `instance_id`, `instructions`, `profile`, `task`, `task_id`, `workspace_ref` | yes | `core/src/v2/control.rs:127` |
| `issue_grant` | `action`, `parent_grant_id`, `resource_scope`, `subject` | yes | `core/src/v2/control.rs:128` |
| `revoke_grant` | `grant_id` | yes | `core/src/v2/control.rs:129` |
| `reauthorize_operation` | `operation_id` | yes | `core/src/v2/control.rs:130` |
| `reset_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:131` |
| `create_goal` | `deadline`, `id`, `instance_id`, `limits`, `original_request_ref` | yes | `core/src/v2/control.rs:132` |
| `require_checks` | `checks`, `goal_id` | yes | `core/src/v2/control.rs:133` |
| `send_message` | `correlation_id`, `max_inbox`, `recipient`, `text` | yes | `core/src/v2/control.rs:134` |
| `drain_inbox` | `instance_id` | yes | `core/src/v2/control.rs:135` |
| `delegate_task` | `acceptance_refs`, `assignee`, `dependencies`, `description`, `goal_id`, `task_id` | yes | `core/src/v2/control.rs:136` |
| `start_task` | `task_id` | yes | `core/src/v2/control.rs:137` |
| `complete_task` | `result_refs`, `status`, `summary`, `task_id` | yes | `core/src/v2/control.rs:138` |
| `cancel_task` | `reason`, `task_id` | yes | `core/src/v2/control.rs:139` |
| `read_history` | `epoch`, `instance_id`, `limit` | yes | `core/src/v2/control.rs:140` |
| `fire_timer` | `now` | yes | `core/src/v2/control.rs:141` |
| `blocked_report` | — | yes | `core/src/v2/control.rs:142` |
| `submit_input` | `envelope_id`, `instance_id`, `text` | yes | `core/src/v2/control.rs:143` |
| `begin_request` | `est_prompt_tokens`, `instance_id`, `offered_tools`, `request_id`, `request_ref`, `revision`, `surface_authorized` | yes | `core/src/v2/control.rs:144` |
| `record_attempt` | `attempt_id`, `elapsed_ms`, `error_class`, `request_id`, `response_ref`, `status`, `unknown_usage`, `usage` | yes | `core/src/v2/control.rs:145` |
| `begin_compression` | `est_prompt_tokens`, `instance_id`, `request_id`, `request_ref` | yes | `core/src/v2/control.rs:146` |
| `compress_context` | `attempt_id`, `instance_id`, `keep_ids`, `request_id`, `summary` | yes | `core/src/v2/control.rs:147` |
| `fail_compression` | `reason`, `request_id` | yes | `core/src/v2/control.rs:148` |
| `import_response` | `completion`, `decision_id`, `entry`, `grant_revision`, `intents`, `notes`, `request_id`, `wait` | yes | `core/src/v2/control.rs:149` |
| `dispatch_operation` | `approval_required`, `operation_id`, `permission_revision` | no | `core/src/v2/control.rs:150` |
| `complete_operation` | `operation_id`, `receipt`, `status` | no | `core/src/v2/control.rs:151` |
| `cancel_operation` | `operation_id`, `reason` | yes | `core/src/v2/control.rs:152` |
| `approve` | `approval_id`, `expires_at` | yes | `core/src/v2/control.rs:153` |
| `deny` | `approval_id` | yes | `core/src/v2/control.rs:154` |
| `fail_request` | `park`, `reason`, `request_id` | no | `core/src/v2/control.rs:155` |
| `cancel_request` | `reason`, `request_id` | yes | `core/src/v2/control.rs:156` |
| `interrupt_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:157` |
| `complete_goal` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:158` |
| `register_check_runs` | — | yes | `core/src/v2/control.rs:159` |
| `register_verification` | — | yes | `core/src/v2/control.rs:160` |
| `repair_completion` | `failures`, `goal_id`, `instance_id`, `round` | yes | `core/src/v2/control.rs:161` |
| `block_goal` | `goal_id`, `instance_id`, `reason` | yes | `core/src/v2/control.rs:162` |
| `cancel_goal` | `goal_id` | yes | `core/src/v2/control.rs:163` |
| `close_completion` | `instance_id` | no | `core/src/v2/control.rs:164` |
| `artifact_abandon` | `id` | yes | `core/src/v2/control.rs:165` |
| `set_lifecycle` | `instance_id`, `lifecycle`, `reason` | yes | `core/src/v2/control.rs:166` |
| `artifact_stage` | `digest`, `id`, `kind`, `owner_ref`, `owner_scope`, `size`, `storage_ref` | no | `core/src/v2/control.rs:167` |
| `artifact_publish` | `id` | no | `core/src/v2/control.rs:168` |
| `artifact_gc_claim` | `limit` | no | `core/src/v2/control.rs:169` |
| `artifact_collect` | `id` | no | `core/src/v2/control.rs:170` |
| `prune_history` | `days`, `evidence`, `now` | yes | `core/src/v2/control.rs:171` |
| `fork_reset` | `keep_instance`, `source_root`, `target_root` | yes | `core/src/v2/control.rs:172` |

### Every field the protocol carries in one object, and where it is built

The rows a client reads (`instances[]`, `tasks[]`, `approvals[]`, `grants[]`, `entries[]`), a
checkpoint's `goal` object and the greeting/reply envelopes are all built as `json!({ … })`
literals in `engine/src/v2/daemon.rs`; this table is that list. A row is one literal.

| Built at | Fields |
|---|---|
| `run_scheduler` | `id`, `instance_id` |
| `serve_client` | `server`, `protocol_version`, `session_id`, `state_root`, `permissions`, `workspace` |
| `serve_client` | `request_id`, `ok`, `error` |
| `handle` | `request_id`, `ok`, `result` |
| `handle` | `request_id`, `ok`, `error` |
| `handle` | — |
| `handle` | `stopping`, `state_root` |
| `read_snapshot` | `id`, `lifecycle`, `phase`, `model`, `reason` |
| `read_snapshot` | `instances`, `goal` |
| `read_snapshot` | `status`, `known_usage`, `unknown_usage`, `limits`, `deadline` |
| `read_events` | `sequence`, `kind`, `scope`, `payload` |
| `apply_session_goal_limits` | — |
| `previews` arm | `previews` |
| `checkpoint` arm | `snapshot`, `watermark` |
| `events` arm | `events`, `watermark`, `resync_required` |
| `history` arm | `epoch`, `idx`, `kind`, `envelope_id`, `message` |
| `history` arm | `instance_id`, `entries` |
| `surfaces` arm | `request_id`, `instance_id`, `epoch`, `goal_id`, `kind`, `status`, `offered_tools`, `surface_authorized` |
| `surfaces` arm | `surfaces` |
| `tasks` arm | `id`, `goal_id`, `assignee`, `status` |
| `tasks` arm | `tasks` |
| `approvals` arm | `id`, `operation_id`, `tool`, `preview` |
| `approvals` arm | `approvals` |
| `goals` arm | `id`, `status`, `deadline`, `limits`, `known_usage`, `unknown_usage`, `attached_instances` |
| `goals` arm | — |
| `goals` arm | — |
| `goals` arm | `goals` |
| `grants` arm | `id`, `issuer`, `subject`, `action`, `resource_scope`, `parent_grant_id`, `revoked` |
| `grants` arm | `grants`, `revision` |

<!-- generated: end -->
