# TeamAgents user guide

This guide describes current source behavior. For installation see [INSTALL.md](INSTALL.md); for evidence and
open obligations see [ACCEPTANCE.md](ACCEPTANCE.md).

## 1. Quick start

```bash
teamagents init
export DEEPSEEK_API_KEY=...          # the variable named by your model profile's api_key_env
teamagents doctor                    # config, credentials, state root and host tools
teamagents doctor environment        # read-only host tool discovery
teamagents                           # open or join the TUI session
```

Use `teamagents --help` for the overview, `teamagents sessions --help` for a command family, and
`teamagents sessions delete --help` for its exact options. Options from another subcommand are refused before
state is accessed. In particular, `--dry-run` belongs to session pruning; session deletion has no preview mode.
MCP process arguments preserve dash-leading values, for example `mcp add --name service --command python3 --arg -u`.
Update actions are exclusive; combining status, check, rollback or application fails explicitly.

### 1.1 Headless runs (`teamagents exec`)

One daemon owns one session. `teamagents` starts it if needed and attaches the TUI. Exiting the TUI does not stop
the session. For one headless input:

```bash
teamagents exec --json --timeout 180 "Summarize the current project state"
```

`exec` returns 0 for a completed/replied run, 1 for failure or unfinished work, 2 for usage/infrastructure,
3 for approval required and 124 for client timeout. Timeout is not proof that no side effect occurred.
Use `exec -- --json` to send the literal text `--json`; everything after `--` is an operand.
`--check CMD` runs an external check after the run, while `--accept ID=CMD` supplies a runtime completion gate.
`--json` prints a final report; `--stream-json` prints incremental JSONL events followed by the report.

### 1.2 The JSON reports

The machine-readable top-level report fields are:

| Report | Fields |
|---|---|
| `exec` | `acceptance`, `acceptance_work`, `action`, `approval`, `approval_id`, `approvals`, `artifacts`, `branch`, `bytes`, `capability_requests`, `collected`, `count`, `decision`, `delegable`, `end`, `entries`, `envelope_id`, `failed`, `failure`, `freed_bytes`, `grant`, `grant_id`, `grants`, `input_queued`, `instance`, `instance_id`, `instance_lifecycle`, `instances`, `lifecycle`, `limit`, `merged`, `output`, `parent_grant_id`, `permissions`, `phase`, `project`, `reply`, `reply_source`, `request_id`, `resource_scope`, `result`, `revision`, `revoked`, `runners`, `schema_version`, `session_id`, `session_workspace`, `snapshot`, `state_root`, `subject`, `task`, `task_id`, `tasks`, `usage`, `verification`, `verification_path`, `watermark`, `work_status`, `workspace` |

| Code | Meaning |
|---|---|
| `0` | Completed or informational reply, with any required checks passing |
| `1` | Failed, blocked, cancelled or unfinished work |
| `2` | Usage, configuration or infrastructure error |
| `3` | Concrete approval required |
| `124` | Client-side timeout; this alone does not prove absence of side effects |

`end` distinguishes completed/replied, unfinished/failed, approval-required and timeout outcomes. `failure` and
`reply` are content, not proof of completion. `usage` carries actual and unknown usage. `verification` carries
required-check results. `snapshot`, `watermark`, `session_id` and `state_root` identify the authoritative read
position. `input_queued` records delivery rather than model or task completion.

## 2. Configuration

The default model profile is `leader_main`; edit `config.toml` rather than expecting `init` to merge new keys.
The default config path is `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`.
The product reads repository-local configuration under an explicit trust rule; it contributes nothing until you opt in. Its permissions,
hooks, checks, limits, retention and update policy never silently override user configuration.
See [CONFIG.md](CONFIG.md) for every field and [INSTALL.md](INSTALL.md) for storage locations.
Startup `-c key=value` overrides include user-only permission settings. A running daemon keeps its configuration;
joining clients report unapplied model/config settings. Use `instances model` to select a configured key for a
new request. Selecting another profile changes its endpoint, protocol and credential reference together; use
an exact key when two profiles share the same wire model name. This is an explicit switch, not automatic routing.

For native Gemini text and tool use, set `protocol = "gemini"`, the Google model ID, and
`api_key_env = "GEMINI_API_KEY"` on a profile. The default API root is
`https://generativelanguage.googleapis.com/v1beta`. Declare `images = true` only for a model that accepts
image **input**. API-key transport is not account login, image generation, or evidence of access to a model.

### Refresh a model directory (Linux)

Opt in **in your user config**. `catalog_sources` maps a namespace to a user-defined Chat Completions profile
with an explicit API root. Refresh sends one GET to that root's `/models`, using only the seed's credential
reference, no prompt and no inference. HTTPS is required except for loopback IPs. Redirects, incomplete
pages, malformed or empty directories do not replace the previous snapshot. An ID-only `/models` response
is insufficient: this adapter reads [OpenRouter-style metadata](https://openrouter.ai/docs/guides/overview/models)
for text input/output, tool support, image input and context length. Other directory formats need adapters.

```toml
[catalog_sources]
router = "router_seed"

[models.router_seed]
provider = "openrouter"
protocol = "chat/completions"
model = "your-chosen-model-id"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
```

```bash
teamagents catalog refresh --name router --json
teamagents catalog list --name router --json
teamagents catalog rollback --name router --json
```

Select a discovered model by its exact `router::<wire-id>` key. Discovery never sets the default model.
Generated entries inherit the seed's destination, protocol, credential reference, timeout and retry policy,
not its price or generation options. A full user profile at the same key takes precedence; no user TOML is
rewritten. A missing row stays in the directory with `missing: true`; it is not evidence that inference will
succeed. Refresh reports before/after metadata per change. The private SQLite cache under `$XDG_STATE_HOME`
(default `~/.local/state/teamagents-catalog/`) keeps the latest eight changed snapshots per source identity.
Rollback publishes the previous snapshot. New requests in a running session read current metadata;
retries never silently acquire a new destination or body. Editing the seed/config itself still requires a restart.

For daily refresh independent of sessions, see the [Linux timer setup](../examples/systemd/README.md).
The timer is not installed or enabled automatically.

<!-- maps: examples/systemd/README.md reader=user -->

### Manage models on an existing local llama.cpp router

Add the allowlist **before the first TOML table** in your user config. Each key must name a profile in that
same file; a project or discovered profile cannot opt in. Use the model ID advertised by your router.

```toml
local_router_profiles = ["desk"]

[models.desk]
provider = "local"
protocol = "chat/completions"
model = "your-local-model"
base_url = "http://127.0.0.1:9931/v1"
# api_key_env = "LOCAL_ROUTER_KEY"
```

```bash
teamagents local-models list --model desk --json
teamagents local-models load --model desk --yes --json
teamagents local-models unload --model desk --yes --json
```

These commands contact the [llama.cpp router](https://github.com/ggml-org/llama.cpp/blob/972d2313bc0bf0a45f634f77d95c9fb03aeab12c/tools/server/README.md)
via a numeric loopback IP only, without using proxies or following redirects. Listing shows IDs and states,
not file paths or process arguments. Changes target only the configured model. Loading refuses if another
model is resident or in transition; unloading can interrupt other clients using this model. Use a dedicated
router if others can change its state between inspection and dispatch. Acknowledgement is not readiness:
list again to inspect the current state. If the router disconnects during dispatch, inspect before retrying.
This CLI does not download or delete weights, launch or stop the router, select a session profile, or run inference.

### 2.1 Acceptance checks (`[[checks]]`)

Required checks come only from user configuration or explicit CLI/work inputs. They run on the host at the
completion boundary and can bind declared inputs and timeouts. A failed check enters bounded repair or blocks
the work; a prose summary cannot override it.

### 2.2 Work limits (`[limits]`)

`max_total_tokens` and `deadline_minutes` bound new requests and reservations across turns, retries, compaction,
review and verification. Unknown usage remains unknown rather than being treated as zero cost. With no configured
ceiling, work continues until the user stops it.

### 2.3 Hooks (`[hooks]`)

`pre_tool` can veto a native tool call and `notify` can report lifecycle/tool events. Hooks run with the user's
authority; project configuration cannot silently install permissions, checks, retention or update policy.

### 2.4 Optional model cost estimates

Set `[models.<name>.price]` with `input_usd_per_million`, `output_usd_per_million` and a `source`
(provider tariff URL or label plus date). Optional `cache_read_usd_per_million` and
`cache_write_usd_per_million` price cached input separately. Input totals include cache reads/writes; these
categories are subtracted before charging uncached input, so tokens are never charged twice.

Prices use USD only, are user supplied, and are frozen at request registration for turns, compression and
Reviewer calls. They are estimates, not provider invoices: discounts, taxes, image-specific charges and
variable cache-duration tariffs require an appropriate tariff and are not inferred. Ambiguous raw model IDs
shared by several profiles are left unpriced. Missing prices, required cache rates or usage stay unknown.
Historical rows are never retroactively priced after a configuration change. Token limits remain independent;
there is no monetary spending cap.

`exec` prints the known estimate and any unpriced attempt count. Its `usage.cost` has `currency`,
`known_estimated_usd`, `unpriced_attempts` and `complete`. Request registration positions select Leader and
compression calls between this input and its outcome. New member calls in the same billing work during that
interval also count; Reviewer calls follow their originating operation. Earlier requests and other works do
not enter the subtotal merely because their receipts arrive during the run. Batched inputs and overlapping
follow-ups can share calls, so summing overlapping exec reports is not a unique session bill. Open calls,
unreceipted closed calls and uncertain cancellation prevent a complete-cost claim. The client caps accounting
at 10,000 registrations and a 16 MiB bookkeeping budget. Exceeding either bound returns an explicit error
with incomplete outcome/usage; session export retains the authoritative records for audit.
The older `usage.unpriced_attempts` counts missing **token usage**, independently of missing monetary prices.
The daemon `work_scopes` read exposes each work's `cost`, including later-attributed admission calls,
`pending_requests` and `usage_without_receipt` (for example, inherited usage without local tariff receipts).
A partial known subtotal is never labelled a complete cost. Session exports preserve request tariffs and
attempt estimates for audit.

## 3. Team workspace, conversation and editing

The Herdr-inspired workspace shows members and their recorded state beside the Leader conversation. At 96
columns the member sidebar becomes persistent; at 132 columns and sufficient height, Chat also has an inspector.
Narrow terminals use one content pane. The composer always names its Leader recipient, even while viewing a member.

- F6 cycles composer, team and inspection focus. In the team list, Up/Down selects a member and Enter opens details.
  `h` opens read-only member history, `d` returns to details, and Esc returns to the draft. Mouse selection uses
  the same member rows and view tabs. Navigation preserves the draft and staged attachments.
- Alt+1 opens Chat, Alt+2 Tasks, Alt+3 Activity and Alt+4 Inspect. The task view includes recorded terminal outcomes;
  a task result or cancellation does not establish process termination.
- Inspect shows saved reasons, work state, next steps, waits and operations. Member history has its own scrolling,
  streamed preview and media. Its newest 400 entries are loaded only while visible; `/sessions` offers older
  history. Read failures keep refresh debt and a visible stale warning until a valid reply arrives.
- Activity retains the newest 200 observed event summaries since attachment. Reconnect gaps are possible; use
  session export for a full audit. User-visible member history never enters Leader context or the shared tree.

In the composer:

- Enter submits; Shift+Enter inserts a newline. Up/Down recalls input. PageUp/PageDown and the mouse wheel scroll.
- Ctrl+Left/Right or Alt+Left/Right moves by word; Ctrl+B/F moves by character. Ctrl+W or Alt+Backspace deletes
  a word. Ctrl+U/K deletes to line start/end, and Ctrl+Y restores the last deletion.
- Ctrl+E opens the current draft in `VISUAL`, falling back to `EDITOR`. Saving with exit 0 replaces the draft;
  cancellation, nonzero exit or read failure restores it. `/edit [TEXT]` opens the editor with optional exact
  text; `/edit` alone starts an empty draft. Text or paths ending in `/edit` have no special meaning.
- `/help` shows current controls, environment and diagnostics. `?` on an empty draft opens the shortcut overlay.
- Slash completion edits only the draft. An explicit second Enter executes the completed command.
- `/attach PATH` stages a workspace file; `/detach INDEX` removes one. `@PATH` followed by Tab offers workspace
  path completion. Attachments are sent only with your message, and symlinks are refused.
- Ctrl+O expands/folds tool previews. Ctrl+G switches image/diagram previews and source. Graphics mode comes from
  `TEAMAGENTS_GRAPHICS=auto|kitty|blocks|off`.

Local commands are `/help`, `/sessions`, `/interrupt`, `/quit`, `/model`, `/attach`, `/detach`, `/queue`, `/tree`,
`/edit`, `/team`, `/chat`, `/tasks`, `/activity` and `/inspect`. They never become model input.
`/team` focuses member selection; the other new view commands open the named view.

## 4. Approvals and lifecycle

Ctrl+A focuses pending approvals. `a` approves once, `d` denies, and `Esc` returns to the draft. Approval arrival
does not steal typing focus. Selection follows the approval ID across refreshes; if that approval disappears,
focus returns to the draft. Decision keys require a plain press. Repeat events cannot confirm approvals, apply
profiles or switch modes. Ctrl+M and Ctrl+Enter both toggle the model menu; plain Enter applies the selection.
An approval binds one operation, argument hash and permission revision.

Ctrl+C or `/interrupt` cancels the Leader's in-flight model request. Running tools and delegated work continue.
Ctrl+Q or `/quit` detaches the TUI while the daemon continues. A paused or refused leader does not silently accept
new input; use the reported recovery command.

Task cancellation releases the delegator but does not prove command termination. Instance termination is a
separate user-only operation. Use `teamagents instances` and `teamagents runners` to inspect actual state before
taking over a shared workspace.

## 5. Session exploration tree

Type `/tree` to inspect the single session-level exploration tree. It shows revision, freshness, source
watermark, focus, node titles/status/kind and references. `teamagents search_tree [--json]` reads the same
projection without starting another daemon.

Ask the Leader to organize it in natural language, for example:

- “Merge duplicate parser investigations and keep their references.”
- “Mark this approach abandoned and explain why.”
- “Focus on the remaining SQLite recovery branch.”
- “Split the current question into implementation and verification branches.”

The Leader applies revisioned add/update/move/merge/focus patches. Ordered operations validate against the
preceding result and reject atomically. A merge unions references and reparents children. A focus changes
navigation only: it does not settle work, change grants or undo effects.

Leader compaction advances the previous cumulative tree through newly visible Leader history, including the
retained recent turn. Member-private history is not included. A stale flag means later conversation is not yet
incorporated. An invalid compaction patch emits `search_tree_update_failed` while summary and work continue.

The tree is model-maintained navigation, not verified fact or a second conversation history. Use stored history,
operation receipts and required checks for authoritative outcomes.

## 6. Collaboration and workspaces

The Leader can use `spawn`, `delegate`, `send` and `wait`. Members have no ambient Shell grant; missing
capabilities create a visible request instead of silent execution. Parent revocation cascades to child grants,
and dispatch checks the current revision.

`spawn.workspace` selects `shared`, `isolated` or `git_worktree`. Retirement refuses uncommitted or unmerged
work. A task's terminal state does not prove its process stopped. Before shared-workspace takeover, inspect the
recorded local process group and other stop evidence; escaped descendants and remote effects are not globally
proven stopped.

## 7. Models and providers

Ctrl+M or `/model` opens the Leader's model/reasoning-effort menu. A change applies at the next READY/WAITING
boundary, not to the in-flight request. `teamagents instances model --id ID --model KEY --effort E` addresses
other members. Profiles may use Chat Completions, DeepSeek, Responses or Anthropic and can coexist in one team.

Each profile uses its declared native context window. Compaction preserves the original request, user revisions,
acceptance criteria, open questions and references. Old outputs remain in stored history even when hidden from
the model view.

Transport retries distinguish visible and invisible text. A partial response or late unselected attempt does not
produce tool effects. Provider-reported usage is recorded; unknown usage stays unknown.

## 8. Tools, MCP and Skills

Files are workspace-confined. Shell and acceptance commands run on the host with explicit cwd, execution mode,
exit/signal, duration and output references. Host execution is not OS confinement. Large outputs remain
protected while stored operation receipts or context reference them, even after their owner finishes. Automatic
and offline artifact collection retry failed unlinks and skip `EVIDENCE` roots. Retaining history can therefore
retain its referenced bytes beyond an owner's lifetime; no automatic reference-expiry policy is selected.

Generic MCP bindings negotiate revision **2025-06-18** over stdio and streamable HTTP, with the same operation, budget,
cancellation and receipt paths. A peer returning another revision fails explicitly. `codemode` composes bound
tools in restricted JavaScript without ambient filesystem, network or timers. `tools.mcp_resource` lists/reads
resources, including services with no selected tools; specify `service` when multiple bindings are ambiguous.
Tool discovery follows cursors within one startup-timeout budget, at most 100 pages and 16 MiB total response
bytes. Resource listing returns the server's cursor for the caller to follow.

`teamagents mcp list` inspects configured services. OAuth login/logout uses `teamagents mcp login|logout`;
a server 401 can refresh an unexpired token, including during initialization, without another browser login.
Credentials are bound to the exact resource URL saved at login. Changing that URL requires another login;
selection and refresh check the same locked record. Refresh and retry share the request deadline, including
credential-lock waits. Codemode exposes normalized JavaScript identifiers while preserving remote tool names.
This covers PKCE/public-client flows and local interoperability tests. External authenticated services,
challenge-directed discovery and resumable SSE reconnects are not established by those tests.

For CodeGraph 1.6.3, configure this in the **user** config. The dedicated binding speaks its native
`2024-11-05` stdio revision without a bridge or a generic protocol downgrade:

```toml
[tools.graph]
kind = "mcp"
command = "codegraph"
args = ["serve", "--mcp"]
codegraph_projects = []
```

Use `teamagents mcp list` to check discovery. In codemode, `tools.graph_codegraph_explore({query: "symbol"})`
returns the canonical project, upstream result (including stale-index warnings), and index timestamps.
Timestamps do not prove the index is current. The default project is the member workspace. To let the Leader
query another project, list its exact absolute root and pass `projectPath` in the call. Workers cannot use the
Leader's extra roots. Missing/invalid indexes are refused: no initialization or fallback to a parent project.

For a bound service named `example`, a codemode call can read its public resource:

```javascript
const page = await tools.mcp_resource({action: "list", service: "example"});
text(page);
const resource = await tools.mcp_resource({action: "read", service: "example", uri: "memo://public"});
text(resource);
```

The repository's `fake-mcp-server` test binary supplies this URI; `engine/tests/v2_mcp.rs` exercises it through
the production driver, including a binding that filters out every ordinary tool. Real service URIs come from
that service's resource list.

Web search and fetch use separate bindings. To add Exa fetch alongside DeepSeek, declare:

```toml
[tools.exa_fetch]
kind = "web_fetch"
provider = "exa"
api_key_env = "EXA_API_KEY"
```

With both fetch bindings, the default tries Exa first. A transient transport failure, HTTP 408, 429 or 5xx
allows one attempt through DeepSeek. An explicit provider never falls back. The result names the provider and
reports any fallback reason. The last successful page is cached per member for ten minutes. For long pages,
pass `full_path` and `next_byte_offset` to `read_file` as `path` and `byte_offset`; no second fetch is needed.
The file holds the provider's extracted text, not necessarily the complete original page.

If local DNS returns
fake-IP routing addresses in `198.18.0.0/15`, fetch verifies the hostname through Cloudflare or Google HTTPS DNS.
Private destinations remain refused; unavailable verification returns an error rather than bypassing the guard.
This local check does not establish how the fetch provider handles later DNS changes or redirects.

Skills are read on demand and cannot grant authority. `skill` searches/reads configured roots; relative skills
work from the current workspace. Repository instructions and project trust do not silently widen permissions.
User hooks can veto native tools or send notifications.

## 9. Work, checks and recovery

Work scopes carry original input, budget, deadline and required checks. Every model attempt, retry, compaction,
review and verification shares that budget. Reservations precede requests and settle as known or unknown usage.

Required-check failure triggers bounded repair or `BLOCKED`, never false success. `FAILED` and `CANCELLED` are
also honest outcomes. Progress replies, valid plans and repeated tool results neither settle work nor stop it. A repeat warning
lets the model continue an intentional observation. Without a budget or deadline, such work may continue
until interrupted. Empty replies and invalid actions still have bounded recovery.

After interruption, recovery uses persisted location. Known results are reused; in-flight losses become
`OUTCOME_UNKNOWN`. Side effects are not replayed on a guess. A late response remains in its original epoch and
cannot settle a new assignment.

## 10. Sessions and history

The default state root is `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents/v2`.

Run `teamagents` or `teamagents exec` from a project directory to restore its most recently opened, nonarchived
session. If none exists, a new session uses that directory. `--cwd DIR` selects a different workspace; paths
are stored as absolute paths. Successful opens and switches update recency; listing or reading history does not.
Session-dependent commands such as `status`, `tasks` and `daemon --stop` select the same recent session.

Use `--session ID` or `--state-root PATH` to select an exact session regardless of the launch directory. It keeps
its saved workspace, including after a daemon restart; `--cwd` does not relocate an existing session.
`teamagents sessions new --cwd DIR` creates a session bound to DIR without starting work; open its returned ID
to use it. `--session default` explicitly opens the session stored directly in the base directory. Direct
`daemon` startup uses its explicit/default state root. Exec acceptance checks still run in the caller's
`--cwd` or current directory. A legacy relative Leader workspace is refused because its original base is unknown.

Ctrl+S or `/sessions` opens the session browser for paged history, search, switching and confirmed deletion.
Rename, archive, restore and fork use the `sessions` CLI subcommands. Opening another session replaces the
client only after a successful handshake and
state sync. Drafts remain scoped to their state roots.

Ctrl+P or `/queue` shows pending Leader inputs. Enter edits, Delete asks to cancel, and Esc returns. Pending
edits compare the original envelope, recipient, epoch and revision atomically; they never interrupt a running turn.

Useful commands:

```bash
teamagents status --json
teamagents history --id INSTANCE --limit 400 --json
teamagents search_tree --json
teamagents instances
teamagents tasks
teamagents work_scopes
teamagents sessions list
teamagents sessions search --query TEXT
teamagents sessions export --id ID --output FILE --format json
```

Export streams history, revisions, references and search-tree revisions from one read snapshot and refuses to
overwrite an existing destination. External media remain references; availability is reported explicitly.

## 11. Configuration and diagnostics

The default profile is `leader_main`:

```toml
[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
context_window = 1000000
api_key_env = "DEEPSEEK_API_KEY"
generation_options = { reasoning_effort = "high" }
```

Configuration is documented in [CONFIG.md](CONFIG.md), daemon methods in [PROTOCOL.md](PROTOCOL.md), tools in
[TOOLS.md](TOOLS.md), and events in [EVENTS.md](EVENTS.md).

`doctor` checks config, credentials, state root and host without making model requests. `doctor environment`
discovers tool locations without executing them. `status --json` reports live checkpoint, work, usage, limits and
MCP state. `surface --json` reports the tools offered to requests, not proof the model used them.

Development checks live under `tools/`; see [DEVELOPMENT.md](DEVELOPMENT.md) for `make check`, PTY and formal
commands. Local scripted checks do not replace real-provider, OAuth, public-host or clean-host acceptance.
