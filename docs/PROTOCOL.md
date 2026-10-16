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

**Commands** are executed by the control plane under the caller's identity (`Identity::User`) in one database
transaction, with the permission and budget checks of `core/src/v2/capability.rs`; `identity checked` below means
the handler re-checks who may do this rather than trusting the request.

<!-- generated: begin -->

### Read methods (answered from the session database)

| Method | Parameters | Reply keys | Answered at |
|---|---|---|---|
| `previews` | — | `previews` | `engine/src/v2/daemon.rs:643` |
| `checkpoint` | — | `snapshot`, `watermark` | `engine/src/v2/daemon.rs:649` |
| `events` | `since` | `events`, `resync_required`, `watermark` | `engine/src/v2/daemon.rs:656` |
| `history` | `instance_id`, `limit` | `entries`, `envelope_id`, `epoch`, `idx`, `instance_id`, `kind`, `message`, `workspace` | `engine/src/v2/daemon.rs:665` |
| `surfaces` | `instance_id`, `limit` | `epoch`, `goal_id`, `instance_id`, `kind`, `offered_tools`, `request_id`, `status`, `surface_authorized`, `surfaces` | `engine/src/v2/daemon.rs:698` |
| `inputs` | — | `envelope_id`, `epoch`, `inputs`, `instance_id`, `next_after`, `payload`, `revision`, `sequence` | `engine/src/v2/daemon.rs:730` |
| `tasks` | — | `assignee`, `goal_id`, `id`, `status`, `tasks` | `engine/src/v2/daemon.rs:781` |
| `approvals` | — | `approvals`, `id`, `operation_id`, `preview`, `tool` | `engine/src/v2/daemon.rs:797` |
| `goals` | — | `attached_instances`, `deadline`, `goals`, `id`, `known_usage`, `limits`, `status`, `summary`, `unknown_usage` | `engine/src/v2/daemon.rs:826` |
| `grants` | — | `action`, `grants`, `id`, `issuer`, `parent_grant_id`, `resource_scope`, `revision`, `revoked`, `subject` | `engine/src/v2/daemon.rs:872` |

### Commands (executed by the control plane)

| Method | Parameters the handler reads | Identity checked | Handled at |
|---|---|---|---|
| `create_instance` | `id`, `profile`, `workspace_ref` | yes | `core/src/v2/control.rs:159` |
| `spawn_instance` | `acceptance_refs`, `goal_id`, `instance_id`, `instructions`, `profile`, `task`, `task_id`, `workspace_ref` | yes | `core/src/v2/control.rs:160` |
| `issue_grant` | `action`, `parent_grant_id`, `resource_scope`, `subject` | yes | `core/src/v2/control.rs:161` |
| `revoke_grant` | `grant_id` | yes | `core/src/v2/control.rs:162` |
| `reauthorize_operation` | `operation_id` | yes | `core/src/v2/control.rs:163` |
| `reset_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:164` |
| `create_goal` | `deadline`, `id`, `instance_id`, `limits`, `original_request_ref` | yes | `core/src/v2/control.rs:165` |
| `require_checks` | `checks`, `goal_id` | yes | `core/src/v2/control.rs:166` |
| `send_message` | `correlation_id`, `max_inbox`, `recipient`, `text` | yes | `core/src/v2/control.rs:167` |
| `drain_inbox` | `instance_id` | yes | `core/src/v2/control.rs:168` |
| `delegate_task` | `acceptance_refs`, `assignee`, `dependencies`, `description`, `goal_id`, `task_id` | yes | `core/src/v2/control.rs:169` |
| `start_task` | `task_id` | yes | `core/src/v2/control.rs:170` |
| `complete_task` | `result_refs`, `status`, `summary`, `task_id` | yes | `core/src/v2/control.rs:171` |
| `cancel_task` | `reason`, `task_id` | yes | `core/src/v2/control.rs:172` |
| `read_history` | `epoch`, `instance_id`, `limit` | yes | `core/src/v2/control.rs:173` |
| `fire_timer` | `now` | yes | `core/src/v2/control.rs:174` |
| `blocked_report` | — | yes | `core/src/v2/control.rs:175` |
| `submit_input` | `envelope_id`, `instance_id`, `text` | yes | `core/src/v2/control.rs:176` |
| `edit_input` | `envelope_id`, `instance_id`, `revision`, `text` | yes | `core/src/v2/control.rs:177` |
| `cancel_input` | `envelope_id`, `instance_id`, `revision`, `text` | yes | `core/src/v2/control.rs:178` |
| `begin_request` | `est_prompt_tokens`, `instance_id`, `offered_tools`, `request_id`, `request_ref`, `revision`, `surface_authorized` | yes | `core/src/v2/control.rs:179` |
| `record_attempt` | `attempt_id`, `elapsed_ms`, `error_class`, `request_id`, `response_ref`, `status`, `stream`, `unknown_usage`, `usage` | yes | `core/src/v2/control.rs:180` |
| `begin_compression` | `est_prompt_tokens`, `instance_id`, `request_id`, `request_ref` | yes | `core/src/v2/control.rs:181` |
| `compress_context` | `attempt_id`, `instance_id`, `keep_ids`, `request_id`, `summary` | yes | `core/src/v2/control.rs:182` |
| `fail_compression` | `reason`, `request_id` | yes | `core/src/v2/control.rs:183` |
| `import_response` | `completion`, `decision_id`, `entry`, `grant_revision`, `intents`, `notes`, `request_id`, `wait` | yes | `core/src/v2/control.rs:184` |
| `dispatch_operation` | `approval_required`, `operation_id`, `permission_revision` | no | `core/src/v2/control.rs:185` |
| `complete_operation` | `operation_id`, `receipt`, `status` | no | `core/src/v2/control.rs:186` |
| `cancel_operation` | `operation_id`, `reason` | yes | `core/src/v2/control.rs:187` |
| `approve` | `approval_id`, `expires_at` | yes | `core/src/v2/control.rs:188` |
| `deny` | `approval_id` | yes | `core/src/v2/control.rs:189` |
| `fail_request` | `park`, `reason`, `request_id` | no | `core/src/v2/control.rs:190` |
| `cancel_request` | `reason`, `request_id` | yes | `core/src/v2/control.rs:191` |
| `interrupt_instance` | `instance_id`, `reason` | yes | `core/src/v2/control.rs:192` |
| `complete_goal` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:193` |
| `register_check_runs` | — | yes | `core/src/v2/control.rs:194` |
| `register_verification` | — | yes | `core/src/v2/control.rs:195` |
| `repair_completion` | `failures`, `goal_id`, `instance_id`, `round` | yes | `core/src/v2/control.rs:196` |
| `block_goal` | `goal_id`, `instance_id`, `reason` | yes | `core/src/v2/control.rs:197` |
| `deadline_warning` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:198` |
| `goal_checks_briefing` | `goal_id`, `instance_id` | yes | `core/src/v2/control.rs:199` |
| `cancel_goal` | `goal_id` | yes | `core/src/v2/control.rs:200` |
| `close_completion` | `instance_id` | no | `core/src/v2/control.rs:201` |
| `artifact_abandon` | `id` | yes | `core/src/v2/control.rs:202` |
| `set_lifecycle` | `instance_id`, `lifecycle`, `reason` | yes | `core/src/v2/control.rs:203` |
| `artifact_stage` | `digest`, `id`, `kind`, `owner_ref`, `owner_scope`, `size`, `storage_ref` | no | `core/src/v2/control.rs:204` |
| `artifact_publish` | `id` | no | `core/src/v2/control.rs:205` |
| `artifact_release_spent_owners` | `days`, `limit` | no | `core/src/v2/control.rs:206` |
| `artifact_gc_claim` | `limit` | no | `core/src/v2/control.rs:207` |
| `artifact_collect` | `id` | no | `core/src/v2/control.rs:208` |
| `prune_history` | `days`, `evidence`, `now` | yes | `core/src/v2/control.rs:209` |
| `fork_reset` | `keep_instance`, `source_root`, `target_root` | yes | `core/src/v2/control.rs:210` |

### Every field the protocol carries in one object, and where it is built

The rows a client reads (`instances[]`, `tasks[]`, `approvals[]`, `grants[]`, `entries[]`), a
checkpoint's `goal` object and the greeting/reply envelopes are all built as `json!({ … })`
literals in `engine/src/v2/daemon.rs`; this table is that list. A row is one literal.

| Built at | Fields |
|---|---|
| `serve` | `id`, `name`, `root`, `active` |
| `mcp_rows` | `name`, `transport`, `required`, `oauth`, `credential` |
| `run_scheduler` | `id`, `instance_id` |
| `serve_client` | `server`, `protocol_version`, `session_id`, `state_root`, `permissions`, `workspace` |
| `serve_client` | `request_id`, `ok`, `error` |
| `handle` | `request_id`, `ok`, `result` |
| `handle` | `request_id`, `ok`, `error` |
| `handle` | — |
| `input_paths` arm | `stopping`, `state_root` |
| `read_snapshot` | `id`, `lifecycle`, `phase`, `model`, `reason`, `can_run_shell` |
| `read_snapshot` | `instances`, `goal`, `mcp`, `sessions` |
| `read_snapshot` | `status`, `known_usage`, `summary`, `unknown_usage`, `limits`, `deadline` |
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
| `tasks` arm | `id`, `goal_id`, `assignee`, `status` |
| `tasks` arm | `tasks` |
| `approvals` arm | `id`, `operation_id`, `tool`, `preview` |
| `approvals` arm | `approvals` |
| `goals` arm | `id`, `summary`, `status`, `deadline`, `limits`, `known_usage`, `unknown_usage`, `attached_instances` |
| `goals` arm | — |
| `goals` arm | — |
| `goals` arm | `goals` |
| `grants` arm | `id`, `issuer`, `subject`, `action`, `resource_scope`, `parent_grant_id`, `revoked` |
| `grants` arm | `grants`, `revision` |

<!-- generated: end -->
