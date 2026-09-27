# The tool surface: what a model may be offered

Every model request carries a list of functions the model may call, and every call becomes an operation with a
durable receipt (A08/A30) — so this list *is* the capability surface of the product. A member's subset is decided
in three layers:

* **The profile's tools** — the schemas in `reference::basic_tool_schemas(web, skills)`, which a session starts
  from as `reference::session_tool_schemas`: the file, shell and `skill` tools, plus **each** web kind only when
  the config declares a `[tools.*]` binding of it (§5.2: binding is the authorization — a session with no
  `[tools.fetch]`/`[tools.search]` entry offers neither, D-168). The leader's profile ships them by default
  (`docs/USER-GUIDE.md` §5).
* **The instance's grants** — the team tools in `kernel::collaboration_tool_schemas(actions)` appear only for the
  actions the instance holds a grant for (D-61: a spawned worker holds nothing of its own, and `wait` is always
  offered to team instances). The dispatch boundary re-checks regardless of what was offered (§6.1), which is why
  an unoffered call fails closed instead of running.
* **The session's bindings** — `[tools.*]` in the config, including MCP services (D-74). Those are discovered
  from the server at session start and appear as `<service>_<tool>`, so they cannot be listed here; `doctor` shows
  what a session actually bound.

`kernel::builtin_tool_schemas()` is the floor: `finish` and the readback tool are offered to every instance.

The section below is generated from those three functions by `python3 review/tool_catalogue.py --write`, so a
renamed tool, a new parameter or a reworded model-facing description fails `make hygiene` until the document is
regenerated. Descriptions are quoted verbatim because they are what the model reads.

<!-- generated: begin -->

### `finish`

*Offered by `kernel::builtin_tool_schemas()` (every instance).*

Finish the current goal exactly once. Must be the only tool call in this response. Report the real outcome; a summary that admits undelivered work must not claim success.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `status` | `string` | yes | — |
| `summary` | `string` | yes | What was done and the final answer. |
| `evidence` | `array` | no | References proving the claims (files, commands, receipts). |
| `unverified` | `array` | no | Claims that could not be verified. |

### `read_history`

*Offered by `kernel::builtin_tool_schemas()` (every instance).*

Page through the full stored output of an earlier tool call (ids appear in masked outputs).

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `tool_call_id` | `string` | yes | — |
| `offset` | `integer` | no | — |
| `limit` | `integer` | no | — |

### `send`

*Offered by `kernel::collaboration_tool_schemas(actions)` (per grant).*

Send a message to another instance. Delivery is queued and applied at the recipient's boundary; a message does not by itself start the recipient's turn.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `recipient` | `string` | yes | Target instance id. |
| `text` | `string` | yes | — |

### `delegate`

*Offered by `kernel::collaboration_tool_schemas(actions)` (per grant).*

Delegate a task to another instance with a narrow return path: the assignee can settle exactly this task back to you, nothing more. The task is charged to the delegating instance's active goal, which must still be open — create a new goal first when the previous one is settled. To be woken by its outcome, wait on {kind:'task',task_id:'<id>'} **with timer_seconds set**: the settlement arrives as a task result, not as a chat message, and an assignee that ends its turn without settling produces no result at all. When that happens, that part is yours again: re-delegate it as a new task or do it yourself, and never report work you did not verify as success.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `assignee` | `string` | yes | Instance id that receives the task. |
| `task_id` | `string` | no | Optional explicit task id; one is generated when omitted. |
| `description` | `string` | yes | — |
| `acceptance_refs` | `array` | no | References the result must satisfy. |

### `spawn`

*Offered by `kernel::collaboration_tool_schemas(actions)` (per grant).*

Create a new instance and atomically register its initial task with the return path. Creation grants no extra connections or file scopes.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `instance_id` | `string` | yes | — |
| `instructions` | `string` | yes | System instructions for the new instance. |
| `model` | `string` | no | Catalog entry for the new instance (a key such as leader_main, or the model name it declares). Omit to inherit this instance's model; a mixed team comes from naming a different entry. |
| `task` | `string` | no | Initial task description; registered with its narrow return path when present. |
| `workspace` | `string` | no | Where the instance works. shared (default) uses the project directory; isolated gets a private directory under the session state root; git_worktree gets its own branch and worktree, and falls back to shared (saying why) when the project is not a clean git repository. |

### `cancel_task`

*Offered by `kernel::collaboration_tool_schemas(actions)` (per grant).*

Close a task you delegated, and only one you delegated: a cancellation is terminal, consumes the assignee's return path, and satisfies a wait on that task. Use it when an assignee has abandoned the part (it ended its turn without settling) — then re-delegate that part or do it yourself — instead of leaving the task open and waiting out a timer. The assignee is told the task was cancelled; its own work is not undone.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `task_id` | `string` | yes | Task id returned by delegate. |
| `reason` | `string` | no | Why it is being closed; the assignee sees this. |

### `wait`

*Offered by `kernel::collaboration_tool_schemas(actions)` (per grant).*

Park this instance until the conditions hold or the timer fires. Must be the only tool call in this response. Conditions: {kind:'message',from?:'<instance>'} — a chat message from that instance (any sender when omitted) has been applied; a delegated task's outcome is a task result, not a chat message, and never satisfies this. {kind:'task',task_id:'<id>'} — that task reached SUCCEEDED, FAILED or CANCELLED; name this one for work you delegated. A task the assignee settled BLOCKED does not satisfy it. {kind:'operation',operation_id:'<id>'}, {kind:'envelope',envelope_id:'<id>'}. Always set timer_seconds when you wait for delegated work: an assignee that ends its turn without settling leaves the task open, no fact will satisfy the condition, and the timer is then the only wake you get. Re-check the task statuses when it fires and settle only what really holds.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `mode` | `string` | yes | — |
| `conditions` | `array` | yes | — |
| `timer_seconds` | `number` | no | Optional deadline from now; the wait closes when it fires. |

### `ls`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

List files in your workspace (path defaults to '.').

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `path` | `string` | no | — |

### `read_file`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Read UTF-8 workspace text, your own /artifacts/ files, or your private /tool-output/ logs in bounded pages. offset is a 1-based line; byte_offset is an absolute byte continuation. Follow next_byte_offset until eof. include_sha256 returns a revision for safe edits.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `path` | `string` | yes | — |
| `offset` | `integer` | no | — |
| `limit` | `integer` | no | — |
| `byte_offset` | `integer` | no | — |
| `include_sha256` | `boolean` | no | — |

### `write_file`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Write a text file in your workspace, creating parent directories. /artifacts/ is this member's own deliverable directory â teammates cannot read it, so put work the team shares in the workspace. /tool-output/ is private and read-only.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `path` | `string` | yes | — |
| `content` | `string` | yes | — |
| `expected_sha256` | `string` | no | — |

### `edit_file`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Replace exactly one occurrence of old_string. Ambiguous matches fail unchanged. Pass expected_sha256 from read_file to reject concurrent changes.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `path` | `string` | yes | — |
| `old_string` | `string` | yes | — |
| `new_string` | `string` | yes | — |
| `expected_sha256` | `string` | no | — |

### `delete`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Delete a file (a directory when recursive=true) from your workspace.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `path` | `string` | yes | — |
| `recursive` | `boolean` | no | — |

### `glob`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Find workspace files matching a glob pattern, e.g. '**/*.py' (max 500 hits).

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `pattern` | `string` | yes | — |

### `grep`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Search workspace files for a pattern; returns matching lines (max 100).

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `pattern` | `string` | yes | — |
| `path` | `string` | no | — |

### `shell`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Run Bash in the current shell_environment mode. approved_scope uses bwrap: network=true needs approval; temporary files/background processes end with the call. full_auto uses the host filesystem/network; services may survive CLI exit. For services redirect stdin/stdout/stderr, record PID, verify in a later call, and stop explicitly. Timeout/cancel kills the active process group. cwd/exports persist separately per mode; a missing cwd skips this call and resets to workspace root. Inspect nonzero exits. Page long output with read_file under /artifacts/ (exec-*.log). /artifacts/ is a virtual file-tool path and belongs to your member alone.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `command` | `string` | yes | — |
| `timeout` | `integer` | no | — |
| `network` | `boolean` | no | — |

### `web_search`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Search the web and return title, source URL, snippet, fetch time (and full content when include_content=true).

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `query` | `string` | yes | — |
| `max_results` | `integer` | no | — |
| `include_content` | `boolean` | no | — |

### `web_fetch`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Fetch a web page and return title, source URL, fetch time and the readable text body (HTML only; capped).

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `url` | `string` | yes | — |
| `max_bytes` | `integer` | no | — |

### `skill`

*Offered by `reference::session_tool_schemas` (the profile's tools; the web half only for the kinds the config declares — §5.2/D-79/D-168).*

Discover and load agent skills. action='search' with query keywords lists matching skills (name â summary); action='read' with a skill name loads its full instructions. Read a skill before applying it.

| Parameter | Type | Required | Meaning |
|---|---|---|---|
| `action` | `string` | yes | — |
| `query` | `string` | no | — |
| `name` | `string` | no | — |

<!-- generated: end -->
