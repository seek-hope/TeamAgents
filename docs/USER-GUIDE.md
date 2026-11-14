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

### 1.1 Headless runs (`teamagents exec`)

One daemon owns one session. `teamagents` starts it if needed and attaches the TUI. Exiting the TUI does not stop
the session. For one headless input:

```bash
teamagents exec --json --timeout 180 "Summarize the current project state"
```

`exec` returns 0 for a completed/replied run, 1 for failure or unfinished work, 2 for usage/infrastructure,
3 for approval required and 124 for client timeout. Timeout is not proof that no side effect occurred.

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

## 3. Conversation and editing

- Enter submits; Shift+Enter inserts a newline. Up/Down recalls input. PageUp/PageDown and the mouse wheel scroll.
- Ctrl+Left/Right or Alt+Left/Right moves by word; Ctrl+B/F moves by character. Ctrl+W or Alt+Backspace deletes
  a word. Ctrl+U/K deletes to line start/end, and Ctrl+Y restores the last deletion.
- Ctrl+E opens the current draft in `VISUAL`, falling back to `EDITOR`. Saving with exit 0 replaces the draft;
  cancellation, nonzero exit or read failure restores it.
- `/help` shows current controls, environment and diagnostics. `?` on an empty draft opens the shortcut overlay.
- Slash completion edits only the draft. An explicit second Enter executes the completed command.
- `/attach PATH` stages a workspace file; `/detach INDEX` removes one. `@PATH` followed by Tab offers workspace
  path completion. Attachments are sent only with your message, and symlinks are refused.
- Ctrl+O expands/folds tool previews. Ctrl+G switches image/diagram previews and source. Graphics mode comes from
  `TEAMAGENTS_GRAPHICS=auto|kitty|blocks|off`.

Local commands are `/help`, `/sessions`, `/interrupt`, `/quit`, `/model`, `/attach`, `/detach`, `/queue`, `/tree`
and `/edit`. They never become model input.

## 4. Approvals and lifecycle

Ctrl+A focuses pending approvals. `a` approves once, `d` denies, and `Esc` returns to the draft. Approval arrival
does not steal typing focus. An approval binds one operation, argument hash and permission revision.

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

MCP negotiates revision **2025-06-18** over stdio and streamable HTTP, with the same operation, budget,
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

Skills are read on demand and cannot grant authority. `skill` searches/reads configured roots; relative skills
work from the current workspace. Repository instructions and project trust do not silently widen permissions.
User hooks can veto native tools or send notifications.

## 9. Work, checks and recovery

Work scopes carry original input, budget, deadline and required checks. Every model attempt, retry, compaction,
review and verification shares that budget. Reservations precede requests and settle as known or unknown usage.

Required-check failure triggers bounded repair or `BLOCKED`, never false success. `FAILED` and `CANCELLED` are
also honest outcomes. A progress reply does not settle work. Empty, plan-only or repeated unproductive recovery
is bounded.

After interruption, recovery uses persisted location. Known results are reused; in-flight losses become
`OUTCOME_UNKNOWN`. Side effects are not replayed on a guess. A late response remains in its original epoch and
cannot settle a new assignment.

## 10. Sessions and history

The default state root is `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents/v2`.

Ctrl+S or `/sessions` opens the session browser. It supports paged history, search, rename, archive, restore,
fork and confirmed deletion. Opening another session replaces the client only after a successful handshake and
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
