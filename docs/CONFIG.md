# The configuration reference

The config file is `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`. `teamagents init` writes a minimal
one (mode `0600`, no credentials, keeps an existing file), `teamagents doctor` reports what it resolved
(credentials, state root, isolation, skills, declared tools, the keys below that this build does not apply), and
the user guide's §2 explains the sections in prose: `[[checks]]` (§2.1), `[limits]` (§2.2), `[hooks]` (§2.3).

Two rules decide whether a key is *honoured*, and both are part of the trust story:

* **Your own config is read, and the repository's is read under one gate.** A repository-local
  `<cwd>/.teamagents/config.toml` is read by the product now — `daemon`, `doctor` and the client all load through
  `config::load_user_config_for` (D-244). It contributes **nothing** until you opt in with
  `[permissions] trust_project = true` in your own config; with the opt-in its `models`, `tools`,
  `skills_paths` and `instruction_files` merge in, and your own definitions of the same name always win.
  `[permissions]` (both `mode` and the trust flag), `hooks`, `checks`, `retention` and `limits` may only ever
  come from your config, trusted or not. `docs/USER-GUIDE.md` §2 says the same in prose, and
  `docs/ACCEPTANCE.md` records what is still not covered (the session's own directory only, and one opt-in for
  every repository). `python3 review/project_config_claim.py` checks that every document agrees with the code on
  this point (D-133, D-244).
* **A key this build accepts but does not apply says so** in the table below and in `doctor` (D-102's
  D-75's `codex_profile` and `[retention]`'s `archived_days`, D-245) — accepted-and-ignored would otherwise
  look exactly like accepted-and-working.
* **A key — or a *value* — this build does not serve is refused, or named by `doctor` before a session can start.**
  At load, with a pointer here (`unknown key … docs/CONFIG.md lists every key this build serves`): at the top
  level, inside every table and inside each `[models.*]`/`[tools.*]` entry — `protocol` must be one this build can
  dispatch (an *empty* one is the historical chat/completions default), `kind` one it can bind, and
  `mcp_execution` one it can honour. In a `doctor` row instead, when the value is one a *session* would fail on
  later and the user can still fix it: `mcp_transport` and a mistyped MCP command (D-74's "where the user can
  still fix it without reading a daemon log"). A typo is a config error, never a silent default — measured before the rule was complete (2026-09-27):
  `skills_pathes = []` left `doctor` green and the path never loaded, and `[permissions] mod = "full_auto"` (a typo
  of `mode`, a *safety* setting) silently ran the session in `approved_scope` (D-161). The one deliberate exception
  is a value that is free-form by design: `generation_options` is a `HashMap` the provider passes through, so its
  keys are the service's, not this build's.

The table is generated from the structs in `core/src/models.rs` by `python3 review/config_reference.py --write`;
`make hygiene` fails when the two drift. Two things to read carefully: the **Absent** column is the value the
field holds when the key is missing, which is not always the *effective* default (an `Option` key is usually read
with `unwrap_or(…)`, so the reader decides — `tool_timeout_s` is unset here and 120 seconds where it is used), and
**Read by** is a search for a *use* of the key (`.key` or `["key"]`) in the crates outside the loader, the
doctor surface and the argv parser — a field declaration of another struct with the same name does not count —
so it names the consumers rather than proving every code path. A key the loader applies when it parses the file
(`deadline_minutes`) shows no reader here for that reason; `doctor` shows what the resolved session carries.
And a key whose struct carries `#[serde(default = "fn")]` shows *that* function's value, not the type's: the loader
calls it when the key is missing, so the column would otherwise be wrong for exactly the keys that have a considered
default (D-239).

<!-- generated: begin -->

### `[models.<name>]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `provider` | `String` | empty | `engine/src/providers/mod.rs`, `engine/src/tools.rs` … (3 files) | The vendor hint: `deepseek` selects that service's defaults (the reasoning echo, the deepseek protocol) while `protocol` is unset, and any other value is a label for a compatible service — the wire is decided by `protocol`/`base_url`, never by this name (D-40, D-229). |
| `protocol` | `String` | "openai" | `engine/src/providers/mod.rs` | — |
| `model` | `String` | empty | `core/src/kernel/instance.rs`, `core/src/kernel/mod.rs` … (13 files) | — |
| `base_url` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs` | — |
| `api_key_env` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs`, `engine/src/tools.rs` | — |
| `timeout` | `i64` | 120 | `core/src/v2/control.rs`, `engine/src/mcp.rs` … (8 files) | — |
| `max_retries` | `i64` | 2 | `engine/src/reference.rs`, `engine/src/v2/driver.rs` … (3 files) | Transport retries the driver may add to one request: the instance that uses this profile retries a transient failure this many times before the turn parks with `transient retries exhausted` (A19). Applied per instance since D-247 (D-240 measured the key accepted and ignored, because one session constant served every member); 0 means one attempt, and a negative value is refused at load. |
| `generation_options` | `HashMap<String, Json>` | empty | `engine/src/providers/mod.rs` | — |
| `context_window` | `Option<u64>` | unset (the reader applies its own) | `core/src/kernel/instance.rs`, `engine/src/providers/anthropic.rs` … (8 files) | Model context window in tokens (drives the /status remaining-context column; None = unknown, shown as "not configured"). |
| `codex_profile` | `Option<String>` | unset (the reader applies its own) | nothing: D-75: refused at load — an external Codex profile is not part of this release | Codex members only: layer `$CODEX_HOME/<name>.config.toml` by running `codex --profile <name> app-server`. **Not implemented in this release** (DESIGN Q12 excludes an external Codex adaptation): no code path reads it, and a config that sets it is refused at load instead of being ignored (D-75) — configure the member directly with `provider`/`protocol`/`base_url`/`api_key_env`. |

### `[tools.<name>]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `kind` | `String` | empty | `core/src/kernel/mod.rs`, `core/src/v2/control.rs` … (15 files) | — |
| `required` | `bool` | false | `core/src/kernel/mod.rs`, `engine/src/bound.rs` … (3 files) | — |
| `provider` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs`, `engine/src/tools.rs` … (3 files) | — |
| `api_key_env` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs`, `engine/src/tools.rs` | — |
| `mcp_server` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | — |
| `mcp_transport` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | — |
| `mcp_execution` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | Local MCP execution boundary: workspace (default) or explicit host. |
| `mcp_network` | `bool` | false | `engine/src/bound.rs` | Network access for workspace-sandboxed MCP processes. |
| `command` | `Option<String>` | unset (the reader applies its own) | `core/src/v2/control.rs`, `engine/src/bound.rs` … (16 files) | The `kind = "mcp"` service's argv over stdio (the default transport) — required for that kind and transport, refused at load otherwise (D-232). |
| `args` | `Vec<String>` | empty | `core/src/kernel/mod.rs`, `core/src/v2/control.rs` … (12 files) | — |
| `url` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs`, `engine/src/reference.rs` … (3 files) | The `kind = "mcp"` service's endpoint over the `http` transport — required there, and an absolute http(s) URL (D-232). |
| `bearer_token_env_var` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | Bearer token for the http transport: names the environment variable the secret is read from — the token itself never lands in this file. |
| `startup_timeout_s` | `Option<u64>` | unset (the reader applies its own) | `engine/src/bound.rs` | initialize/tools/list timeout in seconds (default 60). |
| `tool_timeout_s` | `Option<u64>` | unset (the reader applies its own) | `engine/src/bound.rs` | tools/call timeout in seconds (default 120). |
| `env` | `HashMap<String, String>` | empty | `engine/src/bound.rs`, `engine/src/jobs/mod.rs` … (6 files) | — |
| `tool_names` | `Vec<String>` | empty | `engine/src/bound.rs` | — |

### `top level`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `models` | `HashMap<String, ModelProfile>` | empty | `engine/src/providers/mod.rs`, `engine/src/v2/driver.rs` | — |
| `tools` | `HashMap<String, ToolBinding>` | empty | `core/src/kernel/instance.rs`, `core/src/kernel/mod.rs` … (11 files) | — |
| `skills_paths` | `Vec<String>` | empty | `engine/src/tools.rs` | Directories the `skill` tool searches for `SKILL.md` entries. An entry must exist and be a directory; the default (`~/.agents/skills`) is used only when the key is unset (D-232). |
| `instruction_files` | `Vec<String>` | empty | `engine/src/v2/driver.rs` | Files whose text is appended to **every** member's system prompt (the leader's and each child's), in this order, each under a heading naming the file: read per turn, so an edit lands on the next turn, and a file that cannot be read is named by `doctor` and on the daemon's log rather than skipped in silence (D-102 recorded the promise, D-246 delivers it). |
| `retention` | `Retention` | the Retention default | `engine/src/v2/driver.rs`, `engine/src/v2/supervisor.rs` | — |
| `hooks` | `Hooks` | the Hooks default | `engine/src/hooks.rs`, `engine/src/v2/driver.rs` | — |
| `checks` | `Vec<CheckSpec>` | empty | `core/src/v2/control.rs`, `engine/src/v2/exec.rs` … (4 files) | Acceptance checks the user predefines for every goal (DESIGN §8, Q11): the runtime runs them in the isolated shell at the completion boundary, so a goal cannot be reported as done while a check fails. They are the user's own machine contracts, never conditions a model extracted. |
| `limits` | `GoalLimits` | the GoalLimits default | `engine/src/v2/daemon.rs`, `engine/src/v2/goals.rs` … (3 files) | Usage and wall-clock ceilings every goal this session creates carries (§8, A18/A35): the goal's `max_total_tokens` refuses a new request once the settled usage would pass it, and its deadline refuses one past that moment. Both are the user's own bounds; a session with neither runs until the user stops it (see `docs/USER-GUIDE.md` §2.2). |

### `[limits]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `max_total_tokens` | `Option<u64>` | unset (the reader applies its own) | `core/src/v2/control.rs`, `engine/src/v2/daemon.rs` … (4 files) | Usage ceiling in tokens (provider-reported and unknown usage included). |
| `deadline_minutes` | `Option<u64>` | unset (the reader applies its own) | nothing: applied by the loader: config.rs turns it into each goal's absolute deadline | Wall-clock ceiling in minutes, counted from the moment the goal is created. |

### `[[checks]]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `id` | `String` | empty | `core/src/kernel/instance.rs`, `core/src/v2/control.rs` … (19 files) | Stable id, used in failures, receipts and repair feedback. |
| `command` | `String` | empty | `core/src/v2/control.rs`, `engine/src/bound.rs` … (16 files) | The command, executed through the same shell tool the model uses. |
| `timeout` | `Option<u64>` | unset (the reader applies its own) | `core/src/v2/control.rs`, `engine/src/mcp.rs` … (8 files) | Seconds; absent means the shell tool's own default. |
| `network` | `bool` | false | `core/src/v2/control.rs`, `engine/src/mcp.rs` … (3 files) | Run with network access (the sandbox is offline by default). |
| `inputs` | `Vec<String>` | empty | `core/src/v2/control.rs`, `engine/src/v2/driver.rs` | Workspace-relative inputs the check reads. |

### `[hooks]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `notify` | `Vec<String>` | empty | `engine/src/hooks.rs`, `engine/src/mcp.rs` … (3 files) | argv of the command to run (event name is appended as the last argument, the event JSON arrives on stdin). Empty = no hooks. |
| `pre_tool` | `Vec<String>` | empty | `engine/src/hooks.rs` | argv of a *policy* command run before a native tool executes: exit 0 allows, exit 2 denies (stderr is the reason). Any other outcome allows and only logs, so a broken hook cannot brick the agent. |

### `[retention]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `archived_days` | `u64` | 0 | nothing: D-245: accepted and not applied — one session per state root (A33) means no archived set | **Accepted and not applied** (D-245): this build keeps one session per state root (A33), so there is no "archived sessions" set to walk. It becomes meaningful with multi-session. |
| `history_days` | `u64` | 0 | `engine/src/v2/driver.rs`, `engine/src/v2/supervisor.rs` | Drop applied deliveries and events older than this many days **when a session starts** (its daemon boots), under the guards `verification/tla/V2Retention.tla` pins: the log's head, a pending wait's fact and a non-terminal instance's newest lifecycle event are live references and stay, and a state root marked with the `EVIDENCE` file is never pruned. 0 keeps the full history: events are the audit trail. |

<!-- generated: end -->
