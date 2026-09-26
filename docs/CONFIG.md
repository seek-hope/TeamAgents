# The configuration reference

The config file is `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`. `teamagents init` writes a minimal
one (mode `0600`, no credentials, keeps an existing file), `teamagents doctor` reports what it resolved
(credentials, state root, isolation, skills, declared tools, the keys below that this build does not apply), and
the user guide's §2 explains the sections in prose: `[[checks]]` (§2.1), `[limits]` (§2.2), `[hooks]` (§2.3).

Two rules decide whether a key is *honoured*, and both are part of the trust story:

* **Only your own config is read at all: the project file is not.** The merge loader for a repository-local
  `<cwd>/.teamagents/config.toml` is implemented and unit-tested (`config::load_user_config_for`: project
  `models` and `tools` merge in; `skills_paths`/`instruction_files` need `[permissions] trust_project_tools =
  true`; `hooks`, `retention`, `checks` and `[permissions] mode` may only come from the user config), but **no
  entry point calls it yet** — the daemon, TUI and `exec` load the user config only, so cloning a repository
  cannot change a session today, and this table describes the user config. `docs/USER-GUIDE.md` §2 says the same
  in prose, and the open decision (wiring it changes what a cloned repository can influence) is recorded in
  `docs/ACCEPTANCE.md`'s known gaps. `python3 review/project_config_claim.py` checks that every document agrees
  with the code on this point (D-133).
* **A key this build accepts but does not apply says so** in the table below and in `doctor` (D-102's
  `instruction_files`, D-75's `codex_profile` and the `[retention]` bounds) — accepted-and-ignored would otherwise
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

<!-- generated: begin -->

### `[models.<name>]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `provider` | `String` | empty | `engine/src/providers/mod.rs`, `engine/src/tools.rs` … (3 files) | — |
| `protocol` | `String` | empty | `engine/src/providers/mod.rs` | — |
| `model` | `String` | empty | `core/src/kernel/instance.rs`, `core/src/kernel/mod.rs` … (13 files) | — |
| `base_url` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs` | — |
| `api_key_env` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs`, `engine/src/tools.rs` | — |
| `timeout` | `i64` | 0 | `core/src/v2/control.rs`, `engine/src/mcp.rs` … (8 files) | — |
| `max_retries` | `i64` | 0 | `engine/src/reference.rs`, `engine/src/v2/driver.rs` … (3 files) | — |
| `generation_options` | `HashMap<String, Json>` | empty | `engine/src/providers/mod.rs` | — |
| `context_window` | `Option<u64>` | unset (the reader applies its own) | `core/src/kernel/instance.rs`, `engine/src/providers/anthropic.rs` … (8 files) | Model context window in tokens (drives the /status remaining-context column; None = unknown, shown as "not configured"). |
| `codex_profile` | `Option<String>` | unset (the reader applies its own) | nothing: D-75: refused at load — an external Codex profile is not part of this release | Codex members only: layer `$CODEX_HOME/<name>.config.toml` by running `codex --profile <name> app-server`. **Not implemented in this release** (DESIGN Q12 excludes an external Codex adaptation): no code path reads it, and a config that sets it is refused at load instead of being ignored (D-75) — configure the member directly with `provider`/`protocol`/`base_url`/`api_key_env`. |

### `[tools.<name>]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `kind` | `String` | empty | `core/src/kernel/mod.rs`, `core/src/v2/control.rs` … (12 files) | — |
| `required` | `bool` | false | `engine/src/bound.rs`, `engine/src/tools.rs` | — |
| `provider` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs`, `engine/src/tools.rs` … (3 files) | — |
| `api_key_env` | `Option<String>` | unset (the reader applies its own) | `engine/src/providers/mod.rs`, `engine/src/tools.rs` | — |
| `mcp_server` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | — |
| `mcp_transport` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | — |
| `mcp_execution` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs` | Local MCP execution boundary: workspace (default) or explicit host. |
| `mcp_network` | `bool` | false | `engine/src/bound.rs` | Network access for workspace-sandboxed MCP processes. |
| `command` | `Option<String>` | unset (the reader applies its own) | `core/src/v2/control.rs`, `engine/src/bound.rs` … (13 files) | — |
| `args` | `Vec<String>` | empty | `core/src/kernel/mod.rs`, `core/src/v2/control.rs` … (12 files) | — |
| `url` | `Option<String>` | unset (the reader applies its own) | `engine/src/bound.rs`, `engine/src/reference.rs` … (3 files) | — |
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
| `skills_paths` | `Vec<String>` | empty | `engine/src/tools.rs` | — |
| `instruction_files` | `Vec<String>` | empty | nothing: D-102: declared, validated and reported as not applied; the gap is in ACCEPTANCE | — |
| `retention` | `Retention` | the Retention default | nothing: D-75: the [retention] table is parsed and reported as not applied; retention is a known gap | — |
| `hooks` | `Hooks` | the Hooks default | `engine/src/hooks.rs`, `engine/src/v2/driver.rs` | — |
| `checks` | `Vec<CheckSpec>` | empty | `core/src/v2/control.rs`, `engine/src/v2/exec.rs` … (3 files) | Acceptance checks the user predefines for every goal (DESIGN §8, Q11): the runtime runs them in the isolated shell at the completion boundary, so a goal cannot be reported as done while a check fails. They are the user's own machine contracts, never conditions a model extracted. |
| `limits` | `GoalLimits` | the GoalLimits default | `tui/src/v2app.rs` | Usage and wall-clock ceilings every goal this session creates carries (§8, A18/A35): the goal's `max_total_tokens` refuses a new request once the settled usage would pass it, and its deadline refuses one past that moment. Both are the user's own bounds; a session with neither runs until the user stops it (see `docs/USER-GUIDE.md` §2.2). |

### `[limits]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `max_total_tokens` | `Option<u64>` | unset (the reader applies its own) | `core/src/v2/control.rs`, `tui/src/v2app.rs` | Usage ceiling in tokens (provider-reported and unknown usage included). |
| `deadline_minutes` | `Option<u64>` | unset (the reader applies its own) | nothing: applied by the loader: config.rs turns it into each goal's absolute deadline | Wall-clock ceiling in minutes, counted from the moment the goal is created. |

### `[[checks]]`

| Key | Type | Absent | Read by | Meaning |
|---|---|---|---|---|
| `id` | `String` | empty | `core/src/kernel/instance.rs`, `core/src/v2/control.rs` … (16 files) | Stable id, used in failures, receipts and repair feedback. |
| `command` | `String` | empty | `core/src/v2/control.rs`, `engine/src/bound.rs` … (13 files) | The command, executed through the same shell tool the model uses. |
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
| `archived_days` | `u64` | 0 | nothing: D-75: reported as not applied; retention is a known gap in ACCEPTANCE | Delete archived sessions untouched for this many days when a session is opened. 0 disables it. |
| `history_days` | `u64` | 0 | nothing: D-75: reported as not applied; retention is a known gap in ACCEPTANCE | Drop applied deliveries and events older than this many days from the session database on open. 0 keeps the full history: events are the audit trail. |

<!-- generated: end -->
