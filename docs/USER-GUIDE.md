# TeamAgents user guide

This guide describes current source through D-480. For installation, see [INSTALL.md](INSTALL.md); for evidence
and candidate work, see [ACCEPTANCE.md](ACCEPTANCE.md) and [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md).
Historical decisions and experiments are not instructions for the current product.

## 1. Quick start

```bash
teamagents init                      # write the config (kept if it exists) and prepare the state root
export DEEPSEEK_API_KEY=...          # the environment variable named by api_key_env
teamagents doctor                    # config, credentials, running session, sources and host execution
teamagents doctor environment        # discover host tool locations without executing those tools
teamagents                           # open the TUI (starts this session's daemon when needed)
```

- **One daemon per session**: `teamagents` probes `$XDG_STATE_HOME/teamagents/v2/daemon.sock` and, when it is
  missing or refuses the connection, starts `teamagents daemon` detached and hands the socket to the TUI.
  Quitting the TUI does not stop the session.
- **Minimal fullscreen conversation (D-445)**: one conversation with the Leader, a one-line status,
  the composer and a short hint. There are no member, task or topology panels and no recipient switching.
  Enter sends; Ctrl+J or Shift+Enter inserts a newline. Up/Down recalls input; PageUp/PageDown and the
  wheel read history. Drafts soft-wrap and survive resizing. Kitty's native Shift-drag copies text.
  A slash prefix shows local commands: Up/Down selects, Tab or Enter fills, Esc cancels without clearing
  the draft. Press Enter to submit the filled command. An exact typed command needs only its ordinary Enter.
- **External editor**: Ctrl+E opens the current draft in VISUAL, falling back to EDITOR. Saving and exiting
  successfully replaces the draft; a nonzero exit, missing editor or read failure restores it and reports
  the error. Nothing is sent automatically. Appending /edit to nonempty draft text and pressing Enter opens
  the same editor; /edit alone shows instructions. Staged attachments are separate from the edited text.
  The editor is a trusted local program running with the user's permissions, not a model sandbox tool.
- **Interrupt and quit**: Ctrl+C or /interrupt cancels the Leader's in-flight model request. The receipt
  says whether anything was interrupted. Running tools and delegated work continue; developer controls are
  listed below. Ctrl+Q or /quit quits the TUI while the daemon keeps running.
- **Approvals appear only when needed**: Ctrl+A focuses the approval list; a approves once, d denies,
  Esc returns to the draft. Approval arrival never steals typing focus, and a click only selects a row.
- **Readable conversations (D-447)**: shaded user messages, Leader Markdown, headings, lists, quotes, links,
  fenced code and tables. Code retains indentation; narrow tables become labelled records. Diff lines have
  addition/removal colors. Completed fenced code uses language syntax colors (D-450), including Rust, Python,
  JavaScript/TypeScript, JSON, TOML and Shell. Unknown languages, incomplete fences or parser failures stay
  plain; coloring also falls back above 64 KiB/500 lines or 4096 bytes in one line. Live streaming uses
  the same layout, remains transient and is replaced by history.
  Tool calls and results share a block; received output does not by itself establish success.
  Ctrl+O expands tool previews from four to forty wrapped lines or folds them again. Live tool text is capped
  at 20,000 characters with a notice; full stored results and arguments remain in /sessions.
  `teamagents history --id ID --json` also reads stored messages.
- **Graphics and images (D-448)**: completed Mermaid, SVG and DOT/Graphviz fences render as previews.
  Mermaid includes flow, sequence, class, state and ER diagrams, pie/XY/quadrant charts, Gantt, journey,
  timeline, mindmap and Git graphs. Other syntax depends on the bundled renderer; failures retain a reason
  and source. Markdown images, native image content and view_image results share the same path, including
  session history. Local PNG/JPEG/GIF/WebP/BMP/TIFF/ICO and SVG files are supported; animations show one frame.
  Ctrl+G switches previews/source; switching previews back on reloads referenced files. Automatic mode uses
  Kitty images outside tmux/Zellij; other terminals get Unicode diagrams or colored half-block previews.
  Set `TEAMAGENTS_GRAPHICS=kitty` to force the Kitty protocol, `blocks` for portable output, `off` to start
  with source/reference text, or `auto` for detection. Only use the forced protocol with a compatible terminal.
  Image paths belong to the message author's workspace; /artifacts references belong to that member. Remote
  URLs stay as links, and SVG external/embedded resources are not loaded. Inputs are limited to 8 MiB; graph
  source to 64 KiB/500 lines, with graph complexity limits. Up to sixteen cached previews and eight queued
  renders keep the UI bounded. Only visible previews request decoding; scrolling back reloads evicted
  images without discarding their known layout heights. Images fit at most 120 columns and 36 rows; larger images scale down, and
  scrolling clips them to the conversation. Display limits never rewrite the saved message. Grammar coloring,
  diagrams and image decoding run in terminable Rust display processes: 3 s for media, 6 s for cold
  code grammars, 16 MiB per wire stream, 768 MiB address space and 8 CPU seconds. Linux x86-64/AArch64 helpers
  deny file opens, sockets and child creation after loading fixed public fonts. Unsupported isolation,
  timeout, crash or invalid output preserves source; code stays readable while colors are pending.
  Toggle, resize and session replacement cancel stale work. These are local process bounds, not a guarantee
  about unresponsive filesystems or kernels.
- **Sessions (D-446)**: Ctrl+S or /sessions opens all sessions under this base, including idle and archived
  ones. Up/Down or a click selects a session and reads its stored history without starting it; [ reads an
  older page, ] returns to a newer page, and PageUp/PageDown or the wheel scrolls within the page. Enter
  switches (restoring an archive and starting a daemon when needed); Esc returns. Each session keeps its own
  unsent draft. Press d or Delete, then y to confirm deletion; n or Esc cancels. Deleting the current session
  successfully exits the TUI. Deleting another leaves the current conversation open. Failure stays visible.
  Deletion stops that session and its jobs; member workspaces must first be retired or moved. A default
  session deletion preserves named sessions and shared data. Press r to refresh the list. Press / to enter a
  search query, then Enter searches stored messages. Up/Down/Home/End or PageUp/PageDown select results;
  the preview identifies the session, member, epoch and entry and explicitly shows a snippet. Enter opens
  its session, / edits the query, and Esc cancels a pending search or clears results before closing the page.
  Search never changes the active session or dispatches model input; unreadable sessions are reported.
- **Attachments (D-451)**: type `/attach PATH` to stage a workspace-relative text file or PNG/JPEG/GIF/WebP.
  Tab after `/attach ` queries workspace paths; Up/Down selects, Tab/Enter fills and Esc dismisses without
  sending. Another Enter stages the path; only submitting a message captures bytes. Paths may contain spaces
  or Unicode. `/attach` lists staging; `/detach INDEX` removes an item. At most eight files, 1 MiB per UTF-8
  text file, 5 MiB per image and 16 MiB total. The engine refuses symlink components, traversal, remote paths,
  special files and unsupported binary content. Snapshots retain bytes/digests across edits, retries and
  source deletion. Text is quoted; images require a model declaring `images = true` and stay native image
  content on supported protocols. Drafts retain staged paths across session switches and failed sends.
  Headless equivalent: `teamagents exec --attach 'src/file name.rs' --attach image.png "Review these files"`.
- **Pending inputs (D-451)**: Ctrl+P or `/queue` lists unconsumed Leader inputs. Enter opens a separate text
  editor; Enter saves, Ctrl+J inserts a line and Esc discards the edit. Attachments remain unchanged.
  Delete then y cancels the captured envelope/revision; Esc keeps it. Use r to refresh and n for the next page.
  The conversation draft stays separate. Consumed, cancelled, old-epoch or stale-revision input is refused.
  CLI: `inputs list`, `inputs paths --query PREFIX`, `inputs edit --id ID --revision N --prompt TEXT` or
  `inputs cancel --id ID --revision N` — human-readable by default, JSON with `--json`. A list page holds at
  most 100 rows and 4 MiB of payload; `next_after` continues from the last row, so a caller pages through
  without losing or repeating an input. Byte lengths are checked before payloads are loaded. If the first
  remaining input alone exceeds the budget, the read fails with its envelope id and revision; edit or cancel
  that input to continue. The read never skips or changes it. A cancelled headless input ends with an explicit
  undelivered failure.
  These actions do not interrupt models or stop running tools/delegations. Ordinary queued input enters
  at the next safe request boundary; there is no separate queue that waits for the entire goal to finish.
  D-479 adds an acknowledgement in the conversation: accepted/queued, waiting for the current turn to end,
  received into the Leader's context, cancelled or superseded. Received does not mean the task finished.
  These receipt rows survive checkpoint/history refresh and reconnect within the same TUI process; a new
  TUI does not recreate all old receipts. If sending fails without a confirmed reply, the draft is restored
  and an unchanged retry reuses its input identity. Inspect Ctrl+P/history before editing an uncertain send.
- **Local session export (D-451)**: `teamagents sessions export --id ID --output FILE --format json`
  (or `markdown`) writes a consistent snapshot of every stored member/context/input revision and reference.
  It preserves tool associations, labels unavailable references and never fetches or embeds external bytes.
  Existing destinations are refused. The export is limited to 256 MiB with a 16 MiB record limit; exceeding
  either fails explicitly. Ctrl+C removes unpublished temporary output; a publication that already completed
  returns success. Pruned records cannot be recovered. Export stays in the developer CLI.
- **Model and effort (D-473)**: Ctrl+M or `/model` opens the Leader's menu: Up/Down chooses a `[models]`
  catalog entry, Left/Right the reasoning effort, Enter applies and Esc cancels. The change lands at the next
  READY/WAITING turn; an in-flight turn keeps its profile. On terminals that encode Ctrl+M as Enter, use
  `/model` instead. The catalog comes from the daemon's checkpoint, never from the TUI reading your config.
  The effort list is the selected model's optional `efforts` array, or the built-in
  `minimal, low, medium, high, xhigh, max` when it is absent. If the current level is absent from a newly
  selected model, the menu falls back to the nearest declared level at or below it (e.g. `xhigh` with
  `high, max` becomes `high`); if no declared level is lower, it uses the lowest declared level. The CLI
  `teamagents instances model --effort VALUE` remains an explicit override outside the menu.
- **Client commands**: /help, /sessions, /interrupt, /quit, /model, /attach, /detach, /queue and /edit. The first
  seven appear in the completion menu; /edit is recognized separately. Unknown or retired commands stay
  local and point to `teamagents --help`; they are never sent to the model.
- **Developer CLI**: all management is also available through scriptable commands, using `--state-root PATH` or
  `--session ID` to select the session. `status --json` reads the running session's checkpoint, model,
  goal usage/limits/deadline, bound MCP state and known sessions without starting a daemon. The watermark
  describes the database snapshot; MCP credentials and the session registry are external reads.
  `history [--id INSTANCE] [--limit N] [--json]` reads the newest N stored entries in chronological order
  (default Leader, N=400, range 1..1000); instance ids accept unambiguous prefixes. Messages within that
  bounded tail retain their stored content; this is not a full-session export.

| Developer need | CLI surface |
|---|---|
| Submit work and check results | `exec --json`, `exec --stream-json`, `--check`, `--accept` |
| Inspect progress and conversations | `status`, `history`, `surface` |
| Inspect/control team members | `instances`: send, pause, resume, interrupt, terminate, merge |
| Inspect/control tasks and goals | `tasks`: list, cancel; `goals`: list, open, cancel |
| Inspect/control permissions and approvals | `authority`: list, grant, revoke, requests, approve, deny; `approvals`: list, approve, deny |
| Sessions and recurring work | `sessions`: list, open, history, export, new, fork, rename, restore, search, archive, delete, prune; `automations` |
| MCP, configuration and diagnostics | `mcp`: list, add, remove, login, logout; `init`, `doctor`, `-c key=value` |
| Processes and stored artifacts | `daemon --stop`, `runners`, `artifacts` |

- **Stop a session**: `teamagents daemon --stop` addresses the daemon through the selected state root's
  socket, not a pid. It returns success if the session stopped or nothing was running, and fails if the socket
  remains after 5 seconds. It accepts no `--cwd`, `--model` or `--full-auto`. Shutdown stops new dispatch and
  preserves committed state; committed runners can finish independently. The next `teamagents` or `exec`
  starts a fresh daemon over the same database.
- **Headless use**: `teamagents exec [--json] [--timeout SEC] [--check CMD] [--accept ID=CMD] "prompt"` goes through the same
  daemon and reports the goal's terminal state, the assistant reply or a timeout; the prompt may come from
  stdin (`-`). The full contract is in §1.1.
- **Authority** (D-61): `teamagents authority` lists the session's instances and grants, `teamagents authority
  grant --subject ID --action shell --scope workspace` hands one out and `teamagents authority revoke --grant
  ID` takes it back. This is how a worker the Leader spawned gets the shared-workspace shell (§3); the full
  contract is in §3.1.
- **Temporary config overrides**: repeat `-c key=value` to set dotted keys for one invocation, for example
  `-c limits.tool_output_chars=8000` or `-c models.leader_main.model=deepseek-flash`. Numbers and booleans are
  parsed as TOML; other values are strings. A client starting a daemon passes these overrides to it. Unknown
  or unserved keys are rejected. A running daemon keeps its boot settings; restart it or select another state
  root instead of assuming an override applied.
- **Workspace and launch settings belong to the daemon**: `--cwd DIR` selects the workspace for a new
  session. A client joining a running daemon keeps and prints the existing workspace and settings; another
  `--cwd` does not move it. Stop that daemon, select another state root or choose a named session deliberately.
  Sessions are not automatically selected by repository. Change the Leader's model/effort with Ctrl+M or
  `/model`. The workspace must be an existing directory because file tools need a root and Shell needs an
  initial cwd; invalid paths are rejected before startup.
- **State root**: the default is `$XDG_STATE_HOME/teamagents/v2/` (`~/.local/state/teamagents/v2`). This
  directory holds `session.sqlite`, `daemon.sock` and `daemon.log`; named sessions live below it. The database
  uses WAL and `synchronous=FULL` and carries format/version stamps. Pass the directory to `--state-root`, not
  the database file. A missing directory is created; a regular file or an incompatible path is rejected.

### 1.1 Headless runs (`teamagents exec`)

`exec` is a thin client of the same daemon the TUI uses; it submits one input to the leader and reports what
happened. Diagnostics go to stderr, the outcome to stdout (`--json` prints one JSON object with `end`,
`goal_status`/`reply`, `verification` and the event `watermark`).

| Exit code | Meaning |
|---|---|
| `0` | settled: the goal completed as `SUCCEEDED`, or the leader answered directly |
| `1` | not delivered: the goal settled otherwise, the turn failed permanently, the runtime **refused the request before it began** (a goal budget ceiling, a passed goal deadline — it says so in `failure`), the leader was **terminated while the run was in flight** (it says so in `failure` too), the instance was **reset** while the run was in flight (the epoch move is named), a `--check` command failed, or the run ended `unsettled` (the turn closed with nothing settled — the runtime's own closing word, never a reply) or `undelivered` (the input waited for a boundary and a context reset sealed it before it landed) |
| `3` | an approval is pending — a headless run does not wait for the deadline; decide it with `teamagents approvals` (§4) |
| `124` | the `--timeout` deadline passed with the instance still running (a refusal is not a timeout: it ends the run at once with `1`) |
| `2` | usage or infrastructure: no daemon, no model profile, a leader that is parked or paused |

- **Prompt**: the positional argument, or everything piped into stdin when it is `-`. An empty prompt is a
  usage error.
- **`--stream-json`**: print committed session events as this run observes them, followed by the `--json`
  report. Each line is either `{"type":"event","event":{…}}` or the final
  `{"type":"report","report":{…}}`. Earlier history is drained without being printed. `--json` and
  `--stream-json` are mutually exclusive, and streaming is available only for `exec`. Closing the consumer
  ends the stream without cancelling the run.
- **`--check COMMAND`** (repeatable): run these acceptance commands on the host, in order, after a finished
  turn. They start in the client workspace (`--cwd`, or the current directory). The first failure stops the
  list and makes the run exit `1`. Verdicts go to stderr, `<state root>/verification.json` and the JSON
  `verification` field. Checks do not run after an approval stop, a refused request, an undelivered input or
  the caller's timeout. In those cases `verification_path` is `null` and no new ledger is written.
- **Client checks and goal checks have different jobs**: `[[checks]]` and `--accept` decide whether a goal
  may settle successfully; `--check` decides the exit code of a finished `exec`. A run can therefore have a
  successful goal but fail its client check, or a blocked goal even though its client check passed.
- **`--accept ID=COMMAND`** (repeatable): attach a required check to the active goal before submitting the
  input. The runtime runs it at the completion boundary; failure triggers bounded repair and can ultimately
  settle the goal as `BLOCKED`. Use it when completion depends on the check, for example
  `exec --accept tests="pytest -q" "Fix the failing tests"`. Only a user can attach checks. IDs are merged
  with the goal's existing checks, and the contract cannot change after a check round has started; cancel that
  goal and open another to replace it.
- **Durable memory**: the `memory` tool lets a member `remember` a short note and any later session under
  the same base directory `recall` it (the state root itself for the default session; a named session's base is
  the directory holding `sessions.json`, so every named session under it shares the store). The file is
  `<base>/memory.json`, a versioned append-only list:
  a note is never rewritten or evicted (at 500 notes the store refuses rather than dropping the oldest), each
  note is capped at 2,000 characters and carries the session and member that wrote it, and a recall returns
  at most 20 newest-first matches with their provenance. It is bound like `skills` (D-405).
- **Binding an MCP service**: `teamagents mcp add --name stats --command /usr/bin/python3 --arg server.py
  --execution host` writes one `[tools.stats]` table into the user config (stdio, or `--transport http
  --url …`), and `teamagents mcp remove --name stats` takes it back out. The binding is validated before the
  file is touched, the block is appended rather than the file re-written, and removing the block leaves the
  rest of the config — comments included — alone (D-408).
- **What the configured MCP services offer**: `teamagents mcp list` connects to each `[tools.<name>]`
  binding with `kind = "mcp"` and prints its tools (the model-visible name and the server's one-line
  description); a service that cannot start is named with its reason, and the verb exits `1` when any of
  them failed so a CI job can catch a broken one. This is the surface `doctor` deliberately does not have —
  `doctor` reports the configuration and runs nothing (D-399).
- **`--` ends the option list** (D-390): `teamagents exec -- "- a prompt that starts with a dash"`. Without
  it a prompt whose first character is `-` (a bullet list, a diff, a negative number) is read as an unknown
  flag and refused with the usage text before anything runs. `-` alone still means "read the prompt from
  stdin".
- **An input sent while a turn is running waits for that turn** (D-63): a model request is fixed once it is
  registered, so the input enters the conversation at the next boundary — after that turn's own answer — and
  gets a turn of its own. A headless run reports `input_queued` (and prints
  `queued: a turn was already running, so this input enters after it ends`) instead of pretending it landed;
  the message is never dropped. What such a run *reports* is its own outcome (D-72): the settlement or reply of
  the turn that follows its input, never the earlier turn's — and if a context reset seals the waiting input,
  the run ends `undelivered` (exit 1) instead of waiting out its deadline. The TUI says the same in its note
  line.
- **An approval in a headless run** exits `3` and names the exact call. Decide it with
  `teamagents approvals` / `approvals approve --id …` (D-67, §4.1) and the session goes on — the decision is
  bound to that operation and its arguments, so the run needs no second prompt; the TUI's approvals box shows
  the same call.
- **A parked or paused leader refuses new input** (`2`) instead of queueing work nobody drains: resume it with
  `teamagents instances resume --id i-leader` (§4.2), or use a fresh state
  root.
- When `exec` starts the daemon itself, the daemon's output goes to `<state root>/daemon.log`; if the daemon
  exits while starting, the reason is reported immediately together with that path.

### 1.2 The JSON reports (`--json`)

Many CLI commands print one JSON object with `--json`. The table below is the scripting contract:
`review/exec_report.py` compares these fields with the report objects built by the source.
`exec --stream-json` (D-249) carries the *same* `exec` object, wrapped as the last line
`{"type":"report","report":{…}}` after the session's event lines; the fields below describe it unchanged.
The arrays inside a report (`instances`, `tasks`, `approvals`, `grants`) are rows of the daemon's own views,
whose fields the generated table in `docs/PROTOCOL.md` lists (D-173) — including the `reason` on an instance
row, which `instances` prints when a member is not running (D-165).

| Verb | Fields it adds |
|---|---|
| every report | `session_id`, `state_root` |
| `status` | `schema_version`, `session_id`, `state_root`, `permissions`, `session_workspace`, `watermark`, `snapshot` |
| `history` | `schema_version`, `session_id`, `state_root`, `instance_id`, `limit`, `entries` |
| `instances send` | `session_id`, `state_root`, `instance_id`, `envelope_id`, `result` |
| `exec` | `permissions`, `session_workspace`, `instance_id`, `instance_lifecycle`, `end`, `goal_status`, `reply`, `reply_source`, `failure`, `approval`, `input_queued`, `workspace`, `verification`, `verification_path`, `acceptance`, `acceptance_goal`, `watermark`, `usage` |
| `authority list` | `revision`, `instances`, `grants`, `capability_requests` |
| `authority grant` | `grant_id`, `revision`, `subject`, `action`, `resource_scope`, `parent_grant_id`, `delegable` |
| `authority revoke` | `grant_id`, `grant`, `revoked`, `revision` |
| `authority requests` | `capability_requests` |
| `authority approve` / `deny` | `request_id`, `decision`, `result` |
| `approvals list` | `approvals` |
| `approvals approve` / `deny` | `approval_id`, `decision`, `approval`, `result` |
| `instances list` | `instances` |
| `instances pause` / `resume` / `interrupt` / `terminate` | `instance_id`, `lifecycle`, `instance`, `result` |
| `instances merge` | `instance_id`, `phase`, `lifecycle`, `branch`, `project`, `merged`, `output` |
| `tasks list` | `tasks` |
| `tasks cancel` | `task_id`, `task`, `result` |
| `runners` / `runners stop` | `runners` |
| `artifacts` / `artifacts gc` | `artifacts`, `count`, `bytes`, `collected`, `freed_bytes`, `failed` |

What the field names do not carry:

- `end` is the run's own word for how its turn ended — the `End` values behind §1.1's exit-code table;
  `goal_status` and `reply` are the goal's settlement and the member's answer, and a run reports only what its
  own input produced (D-72); `failure` is the runtime's reason when it refused or failed the turn.
- `artifacts` rows are the state root's catalog (not a daemon view): `id`, `kind`, `size`, `completeness` (`STAGING`, `LIVE`, `DELETING`, `ABANDONED`), `owner_ref` (the fact that owns it, or `null` for an artifact nothing references — the collector's only candidate), `storage_ref` and `bytes_present` (whether those bytes are really on disk). `collected` names what a sweep removed, `freed_bytes` is the sum of their sizes, and `failed` holds the ones whose file deletion failed — their rows stay `DELETING` and the next run retries.
- `runners` rows are the verb's own census of the state root's job directories (not a daemon view): `job_id`, `instance` (the owning member, `null` when a driver's state root *is* the session root), `state` (the journal's own word: `READY`, `RUNNING`, `SUCCEEDED`, `CANCELLED`, `OUTCOME_UNKNOWN`, …), `terminal`, `runner` (`live`, `gone` or `unreachable`), `child_pid`, `service` (how many processes are still in the group the command created, or `null` when the journal cannot anchor it — a job from another boot, or one that never started), `finished_ms`, `exit_code`, and `outcome` — what `stop` did: `retired`, `service stopped: N process(es)`, `no runner`, `no service left`, or `refused: …` with the reason (a runner whose command is running refuses; so does a service stop on a job that is not settled), `null` when listing.
- `verification` is the list of `--check` verdicts, each carrying the command, its ok flag, its exit code and
  its output; `verification_path` is the ledger written next to the session database (`null` when no check ran).
- `acceptance` is what the run attached to the goal with `--accept` (each entry an `id` and a `command`), and
  `acceptance_goal` names the goal it went to (`null` when no `--accept` was given).
- `reply_source` says where `reply` came from (D-431): `model` for the member's own last text, `summary` when
  the model settled the goal **without writing anything** and the reply is the completion summary it recorded
  with its `finish` call, and `""` when there is no reply to report. It exists so a consumer that shows a reply
  to a user can tell the model's words from the runtime's fallback — plain mode says the same thing in words
  ("the model settled the goal without writing anything; its own summary of the work:").
- `usage` is what *this run* cost, as the provider reported it: `prompt_tokens`, `completion_tokens`,
  `total_tokens`, `cache_hit_tokens`, `cache_miss_tokens`, `cache_write_tokens`, `cache_hit_rate`, `requests`,
  and `unpriced_attempts` (attempts the provider never priced, which are **not** counted as free). A zero
  hit/miss pair means the provider did not report a split; it is not a measured miss. Plain mode prints the
  same numbers as one `tokens: …` line, including the cache percentage when the provider reported one. They are
  the numbers the `[limits]` cost keys act on: `tool_output_chars` and `context_keep_chars` (§2.2) change what a
  request carries.
- `watermark` is the event cursor the report was built at, which is what a client resumes from after a
  reconnect (A28).
- `result` is the daemon's own reply to a command, and `instance`/`task`/`approval`/`grant` is the affected row
  as the daemon reported it.

A key this build does not serve is **refused at load with a pointer** (`unknown key … docs/CONFIG.md lists every key this build serves`) rather than ignored, so a typo fails loudly instead of silently leaving its setting at the default (D-161).

## 2. Configuration

Every key this build reads, with its type, what it holds when absent and the files that consume it, is in
[docs/CONFIG.md](CONFIG.md) — generated from the structs and kept in sync by `make hygiene`; `doctor`
reports what a session resolved (including the keys that are accepted but not applied).

The user config is `$XDG_CONFIG_HOME/teamagents/config.toml` (default
`~/.config/teamagents/config.toml`). Credentials are referenced by environment-variable name and never
written into the file:

A repository-local `.teamagents/config.toml` is read too — the daemon, `doctor` and the client all load it —
and the loader looks for it in the working directory and then **up** through its parents, so running from a
package inside a repository uses that repository's config (D-434). As since D-244, it contributes nothing until
you opt in — in one of two ways in your own config:

```toml
[permissions]
trusted_projects = ["/home/you/work/that-repo"]   # exactly these repositories
# or the coarse version, unchanged since D-244:
trust_project = true                              # any repository
```

A `trusted_projects` entry is matched against the repository's **canonical** path, so a symlinked checkout and
its target are the same decision, `/repo` does not trust `/repo/sub` or `/repo-two`, and an entry that resolves
nowhere trusts nothing. With the opt-in the repository's model profiles, tool bindings, `skills_paths` and
`instruction_files` merge in, and your own definitions of the same name win; `[permissions]` (trust and operation-review keys), `[hooks]`, `[[checks]]`, `[retention]`, `[limits]` and `[update]` can only come from your config, so a
cloned repository cannot define hooks, checks or retention policy, raise a ceiling you set or grant itself
trust. Trusted tool bindings can start host programs; inspect a repository before granting it trust. `doctor`'s `project config` row says what the merge did **and which source
trusted it** (`trusted via trusted_projects:/path`), with a `[WARN]` and one reason per refused entry when
nothing landed; the daemon writes the same line into `<state root>/daemon.log`. The rest of this section
describes your own config.

```toml
[models.leader_main]
provider = "deepseek"
protocol = "deepseek"        # deepseek | chat/completions | responses | anthropic | openai (or absent:
                             # chat/completions, with the base URL following provider)
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000     # native window; leave empty when unknown (never guess a small value, D-36)

[tools.web]                  # optional tool binding (web_search / web_fetch / mcp)
kind = "web_search"
provider = "deepseek"
url = "https://api.deepseek.com/v1/tools/search"
api_key_env = "DEEPSEEK_API_KEY"

# Optional second search source; auto calls both configured sources.
# [tools.exa_search]
# kind = "web_search"
# provider = "exa"
# api_key_env = "EXA_API_KEY"

[tools.fetch]
kind = "web_fetch"
provider = "deepseek"
api_key_env = "DEEPSEEK_API_KEY"
```

- `context_window` drives the request budget and the compaction threshold; real models must use their native
  window, and the value and its source are recorded (D-36 in `docs/DECISIONS.md`).
- `teamagents doctor` checks the config item by item, that every model profile's credential resolves, the
  state root (stamp, WAL, read/write) and the paths under it (a directory where `session.sqlite` or
  `daemon.sock` belongs — D-228), the daemon socket path's length (Linux's 108-byte `sun_path` limit,
  which a deep state root can cross — D-227), host execution and that the programs named
  in `[hooks]` are executable; it also lists the `[[checks]]` that will gate every goal. An `sessions/` layout
  from an earlier release is reported explicitly and is never migrated. If a daemon is already running, doctor
  reads its actual workspace/execution and configuration-change status without starting a new session.
- `teamagents doctor environment --cwd DIR [--json]` discovers Rust/Cargo, Node/npm, Python/pip and Git
  locations on the host (D-479, D-480). This is a fresh-shell PATH inspection, not a version/build test or a
  view of a running member's saved exports. Missing tools appear individually; failed or incomplete probes
  exit 1 with an error and recovery hint. No model or discovered executable is run, and no dependencies or
  environment settings are changed. Each probe is limited to 20 seconds and 32 KiB. The command refuses
  --state-root, --session, --full-auto, --model and -c overrides.
- The TUI `/help` and `teamagents status --json` expose the running session's environment. Editing config
  does not hot-reload it: a changed-file warning requests an explicit stop and restart of that daemon.
  An unavailable comparison is reported separately. Preserve the same workspace, state root/session and
  launch options when restarting. Inspect active work first: the daemon stops driving the session, while
  committed runners may finish independently.
- Every web binding needs the credential named by its `api_key_env`. Search accepts `provider = "deepseek"`
  (normally `DEEPSEEK_API_KEY`) and `provider = "exa"` (normally `EXA_API_KEY`); fetch remains DeepSeek-only.
  Without the key — not configured, or the named variable unset — a call answers with a **capability state**
  naming what to configure, and no request leaves the host unauthenticated (DESIGN §7, D-167/D-482). A binding
  with `required = true` that cannot work (missing credential, unserved provider) refuses the member's start,
  so `doctor` reports its row as a **FAIL** and the parked instance carries the reason (D-164/D-165); an optional
  one keeps the capability and the call answers with that state.
  The tools themselves appear only for the kinds you declare (§5.2: binding is the authorization, D-168): a
  config with no web entry offers the model neither `web_search` nor `web_fetch` — `doctor` says so, and adding
  a `[tools.fetch]` entry makes exactly `web_fetch` appear.
- `web_search` keeps its existing `query`, `max_results` (default 10, clamped to 1–50) and `include_content`
  arguments, and adds `ori_question` plus `provider = "auto" | "both" | "deepseek" | "exa"`. Query and original
  question are limited to 128 characters. `auto` uses every bound provider in parallel and merges unique URLs
  with reciprocal rank fusion; `both` requires both providers to be bound; a single-provider argument controls
  cost and source. If one provider fails, successful results are returned with a `warnings` array; if all fail,
  the call returns the classified errors. Transient HTTP/network failures receive two retries with bounded
  backoff. Identical requests share a process-local five-minute, 64-entry cache and concurrent calls coalesce.
  Cache entries contain no credential, and failures are not cached. **With DeepSeek and Exa both bound, `auto`
  can charge both services.** Use one provider and keep `include_content = false` when controlling cost;
  `web_fetch` behavior is unchanged. Exa's optional `env = { search_type = "auto" }` selects its search mode.

`[retention] history_days = N` is applied **when a session starts** (its daemon boots): events and *applied*
deliveries older than N days are dropped from the session database, while the log's head, a pending wait's fact,
a non-terminal instance's newest lifecycle event and your evaluation evidence are never evicted — the rule is
`verification/tla/V2Retention.tla`, with four refuted controls. `history_days = 0` (the default) keeps the full
history. `[retention] archived_days = N` is the same window for **archived sessions**, applied by
`teamagents sessions prune` (D-435): an archived session is removed — its directory and its record — once it has
been archived for more than N days, and `sessions prune --dry-run` shows exactly what `--yes` would remove, with a
line per archived session that stays and the reason (`inside the N-day window`, `no archive time recorded` for a
row archived before that field existed, `carries the EVIDENCE marker`). It is deliberately narrow: only
**archived** sessions are candidates at all (the state root you are working in is never one), a session a daemon
is still serving is kept with the lever to stop it, and the `EVIDENCE` marker wins over the window. `sessions
prune --days N` overrides the configured window; it refuses `--days 0` (a window of nothing keeps everything, as
`history_days = 0` does) and without `--yes` it only tells you what to add. To keep a session's history whatever
the config says, create a file named `EVIDENCE` in its state root — `doctor`'s `retention` row reports both
cases.

### 2.1 Acceptance checks (`[[checks]]`)

These are the machine contracts a goal must satisfy before it can be reported as done (§8, Q11). They come
**only from your own config** — never from a cloned project, because a check is a command that later runs
without an approval prompt:

```toml
[[checks]]
id = "tests"                     # stable id: appears in failures and repair feedback
command = "cargo test --offline" # run through the same shell tool the model uses
inputs = ["src", "Cargo.toml"]   # optional: hashes bind the result to these inputs (A17)
timeout = 900                    # optional seconds (default: the shell tool's own)
```

- The runtime runs them through the host shell at the completion boundary (D-471, D-480), so a goal cannot
  settle while a check fails. A failure returns the work for repair (bounded rounds) and then blocks the goal with the
  failing ids; the check's output lands in the conversation as a tool result.
- `inputs` are workspace-relative paths without `..` escapes. Their hashes are taken when the check runs and
  re-verified before completion, so a check that passed against files that then changed does not count — and a
  check whose own command rewrites its declared input blocks the goal (`bound:stale_inputs`) instead of
  passing. Both halves are measured live (`python3 review/dogfood/stale_check.py`, D-90).
- Checks are not asked through `pre_tool` (you already pre-authorized exactly these commands) and they skip
  the approval gate for the same reason.
- The TUI reports each round by name (`completion check round 1 started: tests, docs`), the failure
  (`completion check round 1 failed: tests (exit), entering a repair turn`) and the final block reason, so a
  goal that is being repaired or parked explains itself instead of looking stuck.
- A broken entry (empty `command`, `timeout = 0`, an escaping `input`) is refused when the config loads, so
  `doctor` and every entry point report it instead of a goal silently never settling.

### 2.2 Goal limits (`[limits]`)

Every goal this session creates can be bounded in cost and in time (D-64):

```toml
[limits]
max_total_tokens = 2000000   # optional: usage ceiling (provider-reported usage + each request's estimate)
deadline_minutes = 45        # optional: wall clock, counted from the moment the goal is created
max_tool_seconds = 60        # optional: the most one *model* tool call may run (a ceiling on what it asks for)
tool_output_chars = 8000     # optional: the most characters of one tool output the model is shown (default 24000)
context_keep_chars = 64000   # optional: how much tool output the request keeps before masking older ones
```

- `max_total_tokens` is the goal's own ceiling: a new request is refused when the settled usage plus the live
  reservations plus the request's estimate would pass it (§8/A18), and the instance parks with the budget as
  the reason instead of overspending. Usage is settled honestly, so a provider that reports later than it
  bills can still overshoot the ceiling afterwards — that is the design's choice, not a bug.
- `deadline_minutes` is turned into the goal's absolute deadline when the session creates it; past that
  moment no new request starts and the instance parks with the deadline as the reason (A35).
- `max_tool_seconds` bounds **one call**, not the session: the model names a timeout for a shell command and
  this caps it, so a single command cannot swallow a turn's window (measured: three calls of 300 s inside one
  775-second trial, D-414). A short ask stays short; only a longer one is cut, and the command's own timeout
  message says so. A user's own `[[checks]] timeout` is **your** number and is never capped. A zero or a value
  over an hour is a config error at load time.
- `tool_output_chars` and `context_keep_chars` are **cost** limits: they change what a request carries, never what
  is stored. The full text of every tool output stays in the session with a readback recipe, so a model that needs
  the rest calls `read_history` for it. Measured on one task set, the defaults cost 61 299 prompt tokens per request
  at the median and the lean pair above 33 387, with the same outcomes (D-426). `context_keep_chars` chooses the
  first mask frontier; D-459 then holds that frontier fixed until compaction so the provider can keep hitting the
  same prefix. New tool outputs therefore stay on the wire until compaction, which is the hard bound; lower the
  value when you pay per prompt token.
- With none of them configured a goal — and therefore the session — runs until you stop it, which is why `doctor`
  says so out loud (`goal limits: none: …`). A zero is a config error at load time.
- These limits come only from **your** config: like `[[checks]]`, a cloned project cannot set them. They are fixed when
  the goal is created; changing them means starting a new session (or a new state root).
- The TUI's status line shows what is in force (`goal ACTIVE · usage 12000/2000000 · ends in 30m`), so a
  configured ceiling is visible and not just enforced.

### 2.3 Hooks (`[hooks]`)

Hooks are **your own programs** (their paths come only from the user config; a model cannot choose them) and
run on the host with your permissions:

```toml
[hooks]
notify = ["/home/you/bin/teamagent-notify.sh"]    # notifications: argv[1] is the event name, JSON on stdin
pre_tool = ["/home/you/bin/policy.sh"]            # policy hook before tool calls
```

- `notify` runs asynchronously and never blocks a turn; it is killed after 10 seconds and failures only reach
  the engine's stderr. The events are `tool_call` (with `tool`/`arguments`/`ok`/`error` — a call your policy
  denied arrives with `ok: false`), `team_action`, `run_completed` and `run_failed`/`run_cancelled` (each
  names its `request_id`, so they fire per **model request**, not per turn), and `run_paused` (the instance
  entered PAUSED).
- `pre_tool` runs synchronously before every native tool call (files/Shell/web/Skills/MCP). Exit code 0
  allows it; **exit code 2 denies it** and the first stderr line becomes the reason handed to the model;
  any other exit code, a spawn failure or a timeout allows the call and logs to stderr — a broken hook never
  stalls the team. Replays after crash recovery are not asked again (the decision was made at first
  dispatch), and the required checks of §2.1 are your own acceptance commands, so they skip `pre_tool`.

## 3. Sessions and teams

- The daemon owns the session: one JSON-lines protocol (`/v1`) over a Unix socket, with the TUI and `exec`
  as thin clients. A reconnect resumes events from the last watermark and commands deduplicate by
  `command_id`. The event log is a documented surface: every kind
  this build emits, with its scope, payload and emitting site, is listed in [docs/EVENTS.md](EVENTS.md)
  (generated from the code and kept in sync by `make hygiene`). The requests and replies themselves
  (framing, the greeting, `command_id` dedup, every read method and command) are catalogued in
  [docs/PROTOCOL.md](PROTOCOL.md), also generated from the dispatchers.
- **The Leader builds the team**: `spawn` creates workers, `delegate` assigns tasks, `send` carries chat
  messages and `wait` waits for results. The session grants the Leader `manage`, `delegate` and `message`;
  grants are checked again at dispatch, and revoked tools disappear from the next model request.
  A delegated result satisfies a `{kind:'task',task_id:'<id>'}` wait when the task becomes
  `SUCCEEDED`/`FAILED`/`CANCELLED`. A `{kind:'message',from:'<worker>'}` wait requires a separate `send`
  message. `BLOCKED`, an idle worker and an unfinished turn do not settle a task wait. Use a timer, inspect
  the task and cancel abandoned work instead of assuming that more waiting will settle it.
- **Delegate independent work that benefits from parallel run-and-fix loops.** Verify the integrated result
  once rather than repeating every worker's checks. Small work that one instance can finish in one pass does
  not need a team. Historical latency comparisons are in [the evaluation report](../review/eval/r2-p6/REPORT.md);
  they do not establish a general success gain.
- **The delegator can cancel its own task** with `cancel_task`; you can use `teamagents tasks cancel`.
  Cancellation is terminal, notifies the assignee and satisfies a task wait, allowing the delegator to
  reassign or finish the work. It does not undo the assignee's side effects or stop its running command.
  `spawn` accepts a model catalog key or model name, so members can use different providers. An unknown model
  or an unbootable instance is reported without stopping the rest of the session.
  Workers do not inherit `shell@workspace`. Grant it directly, or mark a parent grant `delegable` and declare
  `capabilities = ["shell@workspace"]` on the task. The child grant is task-scoped and cannot be redelegated.
  Without a covering parent, the task waits in `WAITING_CAPABILITY` for your decision.
- **Who may run commands**: a member the Leader spawns does not inherit `shell@workspace` (D-61), so the tool
  surface hides `shell` from it (D-60). `teamagents authority` prints the command that would fix it, and
  `teamagents status --json` includes `can_run_shell`, computed using the dispatch gate's covering rule
  (D-441/D-442/D-445).
- **Who runs on what**: every member's model is recorded when it is created and shown in `teamagents instances` (`i-worker · ACTIVE · READY · k3-256k`), which is what makes a team spanning
  two providers (`spawn` with `model = "…"`) readable instead of guesswork (D-69).
- **Workspace policies**: `spawn` accepts `shared` (the project directory), `isolated` (a private directory
  under session state) or `git_worktree` (a separate branch and checkout). If a worktree is unavailable,
  the receipt reports the fallback to `shared`. Termination never deletes a directory with uncommitted or
  unmerged work.
  Merge a worktree member with `teamagents instances merge --id <member>`. The merge is refused while the
  member has a turn in flight, has uncommitted changes or has no worktree branch. Conflicts remain in progress
  for you to resolve or abort with `git merge --abort`; the member branch is not rewritten. After a successful
  merge, the supervisor retires the checkout when its safety checks pass.
- A member whose model ends its turn with plain text (no tool call) and does not settle its task goes
  **idle with the task still `RUNNING`** (D-65: the runtime never reads an outcome out of prose, §8, and it
  never asks the same question twice — that was a turn storm). The delegator's wait stays pending, so
  cancelling the task (`teamagents tasks cancel --id ID`) is what releases it: a cancelled task satisfies the wait and
  the delegator wakes to re-delegate or settle honestly. A `BLOCKED` task does *not* satisfy it.
- Developer intervention uses `teamagents instances` / `teamagents tasks` (§4.2), with `authority`,
  `goals` and `approvals` for the other control-plane objects. The TUI retains approval decisions.
  A goal whose ceiling or deadline is gone refuses new work and parks the instance that asked, so the way out
  is `teamagents goals cancel --id …` (D-341/D-344): the goal settles `CANCELLED` and the instance the refusal
  parked runs again. A goal that has settled cannot be reopened, so the next one is opened with
  `teamagents goals open --id … --attach …`
  (D-267/D-269); the
  session's `[limits]`/`[[checks]]` bound it, applied by the daemon (D-268). A headless run whose leader is
  attached to no active goal says so on **stderr before it submits anything** (D-270), because a turn still runs
  but a delegation inside it is refused — the sentence names the `goals open --attach` lever, so the refusal is
  not read as the model failing.
- Goal and task completion goes through the runtime's completion gate: `finish` only accepts honest
  outcomes, and the required checks you define must really pass.
- **Required checks** come from `[[checks]]` in your config (§2.1) and are carried on the goal itself
  (`create_goal limits.required_checks`, which only the user or the project bootstrap may predefine). The
  runtime runs them through the host shell at the completion boundary (D-471, D-480): a failure sends the
  work into a bounded
  repair loop and then blocks the goal with the failing ids. Their output appears in the conversation like any
  other tool result, so the reason is visible instead of reported as an unexplained "done".
- The headless client's `--check` (§1.1) is a *different* thing: it is your own acceptance command, executed
  by the client after the turn ends, and it only decides that `exec` exits non-zero. A runtime check (above)
  is the stronger contract, because the goal itself cannot settle until it passes.

### 3.1 Authority: who may do what (`teamagents authority`, D-61)

A team is a set of agents with different capabilities, and **you** decide which ones they have. The unit is a
grant: a subject (an instance), an action and the resource it applies to. The session's bootstrap gives the
Leader the three capabilities its own team tools need (`manage`, `delegate`, `message`, D-58) and the
workspace shell; a worker the Leader spawns holds **none** of them (§5.1), which is deliberate — an agent the
team created does not inherit the user's reach. A worker therefore cannot run shell commands until you grant
it `shell@workspace`; it uses the file, web and skill tools (they need their binding, not a grant) until then.
**The list says so now** (D-441): `authority` prints a `capability gaps` line naming each ACTIVE instance that is
missing `shell@workspace` and the command that would give it one, so a user whose members are idle does not have
to infer why. It reports that capability alone on purpose — the other asymmetries are the design, and `task_result`
is issued per task when one is delegated rather than held in advance.

```bash
teamagents authority                                   # instances + grants, and what an instance cannot do
teamagents authority --json                            # the same, with `gaps`: what each ACTIVE instance is
                                                       # missing and the exact `grant` that would fix it (D-441)
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority grant --delegable --subject i-leader --action shell --scope workspace
teamagents authority requests                        # pending capability approvals
teamagents authority approve --request cr-...        # one task-scoped child grant
teamagents authority deny --request cr-...           # cancel the held task, fail closed
teamagents authority revoke --grant g-1a2b3c4d        # the full id or an unambiguous prefix
```

A `delegable` grant is the only thing the Leader may use to issue a child. The child is task-scoped, never
delegable, cannot widen its parent, and dies with the task or its instance; revoking the parent takes the
whole subtree. A task that needs a capability without a covering delegable parent is held in
`WAITING_CAPABILITY` and exposed as a pending approval; it is never dispatched on a guess. The TUI marks such
a member in the status line, and `teamagents status --json` reports the same phase.

The action vocabulary is exactly what some check consults — nothing else can authorize anything:

| Action | Asked over | Meaning |
|---|---|---|
| `shell` | `workspace` | run commands in the shared project directory |
| `manage` | `session` or `instance:<id>` | spawn, and reset/park an instance |
| `message` | `instance:<id>` | send a message to that instance |
| `delegate` | `instance:<id>` | hand a task to that instance |
| `task_result` | `task:<id>` | settle that delegated task back |
| `web` | `domain:<host>` | reach that host with web_search/web_fetch; `session` covers every domain |

`web` grants with `domain:<host>` scopes are an optional allowlist: when the instance has a live `web` grant
whose scope is not `session`, `web_fetch` refuses a host outside the list (exact match, or `*.suffix` for
subdomains). No `web` grant means no allowlist.

`session` is the widest scope: it covers every resource below it, so `--scope session` grants the capability
everywhere in this session, while `--scope instance:i-worker-1` grants it for that peer only. `--parent G`
makes the new grant *derived*: it is then covered by `G` and is revoked with it (a snapshot of the grant tree
rather than a second, independent authority).

What the surface refuses and why:

- **an action outside the table** (`--action shel`) — a typo would create a row nothing reads;
- **a pair no check asks about** (`shell@instance:i-worker-1`): it would be dispatched never, offered never and
  refused never, so the surface says what `shell` *is* asked over instead of writing a grant that looks like
  power and does nothing;
- **a subject that does not exist** is only a *warning*: instance ids are chosen by the spawner, so granting
  ahead of a spawn is legitimate — and a typo is caught the same way, by the note on stderr.

Revocation is final and takes the derived subtree with it. Because every dispatch re-checks the live grants at
its own linearization point (§6.1), a revocation also stops an operation that was authorized when it was
queued, and it is never rewritten into a silent success: the model sees the refusal, and the tool leaves its
surface at its next request.

Exit codes are `0` done, `1` the session refused the command (for example a `--parent` it does not know) and
`2` usage or no session to talk to; `--json` prints the raw report. `authority` is a client of the running
session — it never opens the database — so start the session first (`teamagents`, or any `exec` run) and use
`--state-root PATH` to point at another one. The CLI lists subject, action, scope
and the short grant id; task delegation relationships are available through `teamagents tasks`.

## 4. Permissions and execution

Shell, acceptance commands and local MCP services run on the host with the user's filesystem and network
access. Ordinary Shell calls need no per-command approval. Capability grants still decide which members may
call Shell, file tools enforce workspace paths, hooks may veto calls and self-update requires approval.
There is no built-in OS sandbox or per-command network isolation.

Remove `permissions.sandbox` and `sandbox_*` from old user configs; even `sandbox = "none"` is rejected so an
old isolation choice cannot silently become host execution. Local MCP defaults to `mcp_execution = "host"`,
the only accepted value. Remove `mcp_network` and check-level `network` settings as well. Migration does not
delete sessions, VMs, packages or network policies.

`--full-auto` remains a compatibility flag and has the same effect as an ordinary new boot. An older running
daemon keeps its boot behavior until you stop it with `teamagents daemon --stop` and restart it deliberately.

Bash starts without login profiles and inherits a controlled environment containing host `PATH`/`HOME` and
explicit Rust toolchain locations. Provider credentials are not inherited automatically. Each member keeps
its own cwd and exports; old isolated snapshots are not loaded into host state. Use
`teamagents doctor environment` to discover tools without executing them.

A successful command can leave a service running after the client or daemon exits. Record its pid, redirect
its output and stop it explicitly. Timeout/cancellation targets the active process group, not deliberately
detached sessions. `instances terminate` cancels active work; `tasks cancel` alone does not stop a process.

`approvals_reviewer` and `reviewer_model` apply only to operations that actually require approval. Ordinary
Shell creates none in new production sessions. Capability requests and self-update approval remain user
decisions.

### 4.1 Approving a call from the CLI (D-67)

A pending operation approval is bound to one concrete operation — the hash of the exact command or arguments is part of it, so a modified call needs a new decision
(§6.2).

```bash
teamagents approvals                                   # id, tool and the exact call, per pending approval
teamagents approvals approve --id ap-d-req-4caeb0c6    # once: the parked turn continues with the result
teamagents approvals deny --id ap-d-req-4caeb0c6       # fails closed; the model is told you denied it
```

In the TUI the same decision is one key: a pending approval appears in the approvals box (the status
line and the footer count it), **`Ctrl+A`** focuses that box, `↑`/`↓` pick an entry, **`a`** approves and
**`d`** denies, `Esc` returns to the composer. Ordinary Shell calls create no approval; this surface remains
available for self-update and historical pending operations.

An id must name a *pending* approval (a typo is refused before anything is decided), the full id or an
unambiguous prefix is enough, and `--json` prints the raw report. Exit codes match `teamagents authority`:
`0` done, `1` the session refused it, `2` usage or no session. This is also what makes `teamagents exec` usable
in a script: a run that exits `3` is not a failure, it is a question — answer it and the session continues.

### 4.2 Pausing, resuming and cancelling from the CLI (D-68)

The developer CLI owns instance and task management (D-445), including these recovery paths.
`instances send --id ID --prompt TEXT` preserves direct member input: it returns the accepted/queued
receipt immediately, uses a fresh envelope per invocation and does not wait for completion. Read
`history --id ID` to follow the member's stored messages. Cancelling a task releases the delegator but does not stop its running command;
`instances terminate --id ID --yes` stops its process group. A rejected target is reported by the daemon:

```bash
teamagents instances                    # id, lifecycle, phase, model and reason
teamagents instances send --id i-worker --prompt "Review the failure" --json  # accepted/queued input, not completion
teamagents instances send --id i-worker --prompt "Then run the tests" --after-turn  # queued for after the current turn
teamagents instances model --id i-worker --model deepseek-flash --effort high  # next turn; an in-flight turn keeps its profile
teamagents instances resume --id i-leader      # a parked instance runs again (budget, an unusable model, …)
teamagents instances pause  --id i-worker-1    # stop driving it at the next safe boundary
teamagents instances interrupt --id i-leader   # cancel its running turn; a queued instruction enters next
teamagents instances terminate --id i-worker-1 --yes   # deliberate: retires it and its workspace
teamagents sessions                     # id, state, name, path — the sessions this state root carries (D-364)
teamagents sessions new --name refactor # create a named session beside the default one
teamagents sessions fork --id refactor --name branch   # branch its conversation into a new session (D-365)
teamagents sessions rename --id refactor --name new-name   # rename it
teamagents sessions restore --id refactor       # bring an archived session back
teamagents sessions search --query "tests failed"   # search every session's conversation (read-only, D-368)
teamagents --session refactor           # attach the TUI to it (`exec --session refactor "…"` for one turn)
teamagents sessions archive --id refactor       # move it aside (refused while a daemon runs it)
teamagents sessions delete --id refactor --yes  # remove its directory and history
teamagents sessions prune --dry-run      # what `[retention] archived_days` would remove, and why the rest stays
teamagents sessions prune --yes          # do it (needs the window from the config, or `--days N`)
teamagents automations add --every 60 --prompt "summarize the changelog"   # a schedule the daemon runs (D-367)
teamagents automations                  # id, state, next run, last goal
teamagents automations pause|resume --id ID    # stop it, or re-arm it one period out
teamagents automations remove --id ID --yes    # delete it
teamagents tasks                        # id, status, assignee, goal
teamagents tasks cancel --id t-prose    # releases a delegator waiting on a task that can only wait
teamagents goals                        # the goals this session carries, and what is attached to them
teamagents goals open --id g2 --attach i-leader --check 'tests=python3 -m pytest -q'
                                        # after a goal settles: open the next one, attached to the Leader,
                                        # with your own required checks (§8) and an optional --deadline
                                        # the session's [limits]/[[checks]] bound it too (D-64/D-268)
teamagents surface [--id i-leader]      # what each model request was offered: the tool names it carried and
                                        # whether the surface check authorized them (D-349, the record D-143
                                        # asked for) — a fact in the session, not a log line to infer from
teamagents goals cancel --id g-spent    # close a goal nothing can spend (its ceiling or deadline is gone):
                                        # it settles CANCELLED and the instance its refusal parked runs again
                                        # (D-341/D-344); a goal still carrying running work is refused
```

- **`tasks cancel` is the lever for a stuck delegation**: if a member's model ends its turn without settling
  its task (D-65), the task stays `RUNNING` and the delegator waits; cancelling it **satisfies** that wait
  (a `BLOCKED` task would not), so the delegator wakes and can re-delegate or settle honestly.
- `terminate` needs `--yes`: it retires the instance, its open work is dealt with explicitly, and a workspace
  holding uncommitted or unmerged work is never deleted — the reason is reported instead. Retiring an instance
  while a run is waiting ends that run at once (`1`, with the reason); **pausing** it does not: a pause stops
  new dispatch, so the turn that was running stops at its next boundary and **does not finish** until you
  resume — a headless run against it waits out its own deadline and says so (§4.2, D-98).
  If a run does hit its deadline on a stopped instance, the report names the lifecycle instead of claiming the
  instance is still running.
- **Interrupting one turn (D-363)**: `instances interrupt --id` cancels the instance's running *model request*
  and leaves it `READY`, so an instruction that was queued behind that turn enters at the boundary and gets a
  turn of its own — the interrupt-and-redirect D-63 left open. It settles nothing (the goal and any task stay
  as they were) and it does **not** stop a running tool: a shell command keeps going, and the process-group
  lever for that stays `instances terminate --id … --yes`. Interrupting an instance with no model request in
  flight is reported honestly (`no model request was in flight`), never as a cancellation that did not happen.
- **Named sessions (D-364)**: a state root is the *base* — the default session lives in it directly, and
  `sessions new` creates named ones under `sessions/<id>/`. Each named session is its own state root with its
  own daemon, so several run side by side; `--session ID` attaches the TUI, `exec`, `daemon`, `init` or `doctor`
  to one. `sessions archive` moves a session's directory to `archive/<id>/`, and `sessions delete --yes` removes
  it. Both refuse a running daemon unless deletion explicitly uses `--stop --yes` (D-446). Deleting
  `default` removes only its session data, retaining named sessions and shared memory/configuration.
  `sessions open --id ID --json` restores/starts or joins a session and reports `id`, `socket`, `state_root`.
  `sessions history --id ID [--before N] [--limit N] --json` reads all members/epochs, including archived and
  compaction-covered records, without booting a daemon (default 200 records, limit 1..1000). The response
  carries `id`, chronological `entries` and `next_before` (null at the beginning); pass that exclusive cursor
  as `--before` to read older records. Each entry carries `cursor`, `instance_id`, `epoch`, `idx`, `kind`,
  and the full stored `message`. New appends cannot shift earlier pages. `sessions delete --json` reports
  `id`, `deleted` and `path` only after success; failure exits nonzero, may already have stopped the session,
  and does not claim deletion. `sessions list --json` prints `state_root` and `sessions` (each
  row an `id`, `name`, `path`, `live`, `archived`, `database_bytes`), and `sessions new --json` prints
  `state_root`, `id`, `name`, `path`; these two run before a session is attached, so they carry no `session_id`.
  `sessions fork --id ID [--name NAME]` snapshots the session's database (`VACUUM INTO`, read-only on the source),
  copies its artifacts and resets the copy's execution state in one transaction: the conversation and the goals
  survive, every request/operation/approval/wait/task/envelope and every grant to a removed subject is dropped,
  and the leader is left `ACTIVE`/`READY`. A live source is refused, so the snapshot never races a writer.
  `sessions rename` gives a named session a new display name and `sessions restore` brings an archived one back
  (both refuse the default and a directory a daemon still serves). `sessions search --query TEXT` reads every
  session under the base — default, named and archived — read-only, returning matching conversation lines; a
  database it cannot read is reported by name, never skipped in silence.
- **Automations (D-367)**: `teamagents automations add --every 60 --prompt "…"` stores a schedule the daemon
  runs on its own — each due run opens a goal attached to the Leader and submits the prompt, carrying the
  session's own `[limits]`/`[[checks]]`. `automations` lists id, state, next run and last goal; `pause`/`resume`
  stop or re-arm one (resuming re-arms one period out); `remove --id ID --yes` deletes it. A run never overlaps
  the previous one, and a slot missed while the session was down is coalesced into a single run, not backfilled.
- **Pausing takes effect at a safe boundary**: `PAUSED / TOOLS_PENDING` means an in-flight call must finish;
  `PAUSED / READY` means the instance has paused. `instances` shows lifecycle, phase and reason together.
- **Stopping a command that is already running**: `tasks cancel` is *delegation-level* — the task lands
  `CANCELLED` and the delegator is released, but the assignee's command keeps running until its own tool
  timeout, because the process-group stop (§6.4) is a request against an *operation* and the assignee learns
  about the cancel at its next boundary. The lever that stops the process group is
  **`instances terminate --id … --yes`**: it closes the instance's open execution, the running operation is
  flagged, the runner kills the process group, and the operation lands `CANCELLED` with a receipt whose
  `class` is `cancelled`. Measured with a real model and a real command (`python3
  review/dogfood/cancel.py`, D-88): the effect stops 1.5–6 s after the lever. There is no lighter lever for
  one command today.
- A **reset** has no CLI verb: it is the `reset_instance` command of the session protocol (§9), and it closes
  the epoch's execution — a run waiting on a turn in the old epoch ends at once with exit `1` and a `failure`
  naming the epoch move (D-100).
- Ids must name a listed instance or task (full id or an unambiguous prefix); `--json` prints the raw report
  for scripts. Exit codes: `0` done, `1` the session refused it, `2` usage or no session.

## 5. Skills and MCP

- **Skills and instructions**: `~/.agents/skills` is the default registration root. `skill search/read`
  discovers instructions on demand; Skill text never grants extra execution authority. `skills_paths` and
  `instruction_files` accept `~/…`, and `doctor` reports missing paths.
  Explicit instruction files are read for every member's prompt, in config order. Trusted `AGENTS.md` files
  are discovered from the repository boundary to the cwd, outermost first, after user instruction files;
  untrusted repositories contribute none. `/help` and `status --json` show boot-selected paths and the latest
  Leader prompt-assembly observation, including loaded byte counts or read errors. That observation is empty
  after a driver restart until the next assembly and does not prove provider consumption. Skill scripts still
  need the appropriate host capability and dependencies.
- **Skill resource boundary**: search matches names/descriptions and returns at most ten hits; `read` returns
  at most 32,000 characters of SKILL.md text. Add `path` to read one support file relative to the skill's
  directory: it must stay inside that directory (absolute paths, `..`, symlinks that leave it, directories and
  credential-shaped names such as `.env`/`id_rsa`/`*.pem` are refused) and is truncated at 32,000 characters.
  Skill scripts and assets use their host paths; executing them still requires the corresponding capability.
  A loaded instruction is never a grant to read the host home or execute arbitrary host scripts.
- The tools themselves — every function a model may be offered, with its parameters and the model-facing
  description, split by the layer that decides it (the profile's tools, the instance's grants, the session's
  bindings) — are catalogued in [docs/TOOLS.md](TOOLS.md), generated from the schemas.
- **MCP services load when an instance starts**. A failed `required = true` service parks that instance with
  the runtime's reason; an optional service drops only its capability. Common causes include a missing
  credential, endpoint or command. Fix the cause, then use `teamagents instances resume --id …`.
  Calls use the same permission, budget, cancellation and receipt paths as other tools. If a dispatched remote
  call's outcome is lost during recovery, it is recorded as `OUTCOME_UNKNOWN` and is not replayed.
- **Signing in to a remote service** (D-412): an HTTP MCP server that answers 401 until you present a token it
  issues itself needs a sign-in, not a pasted secret. Bind it with `oauth = true` and sign in once:

  ```toml
  [tools.sentry]
  kind = "mcp"
  mcp_transport = "http"
  url = "https://mcp.example.com/mcp"
  oauth = true                  # the credential comes from `teamagents mcp login`, never from this file
  # oauth_client_id = "..."     # only for a server that cannot register a client dynamically
  # oauth_client_secret_env_var = "MCP_SECRET"   # names the variable, never the value
  # oauth_callback_port = 8765  # only if the server demands an exact pre-registered redirect URI
  ```

  `teamagents mcp login --name sentry` finds the authorization server (through the endpoint's own
  `.well-known/oauth-protected-resource`, then the authorization server's metadata), registers this client
  dynamically unless you named one, prints the authorization URL, opens a browser and waits for the redirect on
  `http://127.0.0.1:<port>/callback`. Over SSH (`--paste`) it prints the URL and reads the *redirected URL* you
  paste back instead; `--no-browser` just prints it, `--timeout SEC` bounds the wait. The flow uses PKCE (S256)
  with a `state` the callback must echo, and the token is stored in
  `<xdg_config_home>/teamagents/mcp-auth.json` — **mode 0600, outside the repository, never printed, never in a
  prompt or a log**; only the variable *name* of a client secret ever appears in the config. An expired token is
  refreshed silently when the server issued a refresh token, a mid-session 401 is answered by one refresh and a
  retry, and a credential that cannot be renewed is a refusal that names the verb to run
  (`teamagents mcp login --name <service>`). `teamagents mcp list` prints each OAuth binding's state
  (`signed in` / `expired (will refresh)` / `not signed in`) before the tools it offers, and
  `teamagents mcp logout --name sentry` deletes the stored credential.
  `teamagents status --json` reports this running session's bound services without loading another
  copy. Use `teamagents goals` for goals and `teamagents sessions` for the registry; reconnect with
  `teamagents --session ID`. A service is
  bound when its instance's driver boots, so a credential stored while a session runs is picked up by
  **pausing and resuming that instance** (`instances pause --id X` then `resume --id X`, or just `resume` when a
  `required` service parked it) — the driver, and with it the toolkit, is rebuilt; the conversation and the goal
  are untouched. A `required` service with no credential parks its instance instead, with the runtime's own
  reason, which is the other path to the same lever.
- **Bind an MCP service in config** with `[tools.<name>] kind = "mcp"`. It can come from user config or a
  trusted project; an untrusted project cannot bind one. Tools are available through codemode under normalized
  service/tool names. `doctor` lists static setup problems without starting the service. MCP resource
  discovery/reading is not implemented.
- **Call MCP tools through codemode**: the model receives one `codemode` tool rather than every bound MCP
  tool as a separate function. JavaScript can batch and filter calls, and only the script's own output enters
  the conversation. Native file/Shell tools are not available inside this composition surface, and the host
  bridge is synchronous rather than a parallel MCP runtime. The script runs in a QuickJS VM whose only capabilities are your
  bound MCP tools (no direct files, network or timers), with a 256 MB heap, an output budget and a per-script deadline
  (default two minutes; `// @options: {"timeout_ms": …, "max_output_tokens": …}` overrides them); `ALL_TOOLS`,
  `searchTools(query)`, `describeTool(name)` and `describeNamespace(service)` find a tool from inside the script,
  and `store`/`load` keep JSON values between codemode calls — persisted under the instance's state root, so a
  restart keeps them. A tool that declares an MCP `outputSchema` gives the script its `structuredContent` rather
  than flattened text. **Your `pre_tool` hook is asked for every nested call**, so a script cannot reach a tool
  you denied; a failed or vetoed nested call rejects with the reason, and an interrupted `codemode` dispatch is
  `OUTCOME_UNKNOWN` and never replayed, exactly like a direct MCP call. `image(...)` accepts a base64 `data:` URL
  (a remote URL is refused), but this codemode path reports an image as a short marker rather than native
  image content. That limit does not apply to explicit image attachments or view_image with an image-capable
  model; those have their own native image path.
- **Local stdio servers run on the host**, with the member workspace as cwd and a controlled inherited
  environment. Explicit binding.env values are layered on top. The default and only mcp_execution value
  is host; old workspace/mcp_network configuration is rejected. The service has the user's filesystem and
  network access. Remote SSH functionality supplied by an MCP service remains available.
- A server that stops answering is bounded the same way (D-108). `startup_timeout_s` (default 60) covers the
  handshake: if `initialize` does not answer inside it, the boot fails, the instance parks with
  `MCP … initialization failed: MCP initialize timed out`, and the server is killed instead of left behind.
  `tool_timeout_s` (default 120) covers one `tools/call`: a call that does not answer inside it is reported to
  the model as a timeout — the turn continues and the model can retry or work around it — rather than blocking
  the member until the server wakes up.

**CodeGraph over stdio MCP.** Install CodeGraph separately and initialize the target project's index explicitly;
TeamAgents never installs it or runs `codegraph init` for you. For a project that already has `.codegraph/`, this
binding discovers `codegraph_codegraph_explore` and makes it callable through codemode:

```toml
[tools.codegraph]
kind = "mcp"
command = "codegraph"
args = ["serve", "--mcp", "--path", "."]
startup_timeout_s = 60
tool_timeout_s = 120
env = { CODEGRAPH_MCP_TOOLS = "explore", CODEGRAPH_TELEMETRY = "0", CODEGRAPH_NO_UPDATE_CHECK = "1", CODEGRAPH_EXPLORE_DEDUP = "0", NO_COLOR = "1" }
```

Use `describeNamespace("codegraph")` to inspect the service and
`await tools.codegraph_codegraph_explore({ query: "login authenticateUser", projectPath: "/absolute/project", maxFiles: 5 })`
to query it. TeamAgents returns the tool's joined text to the script. The default is to reuse CodeGraph's shared
daemon: **do not set `CODEGRAPH_NO_DAEMON=1` while another client owns the project writer**, or the server refuses
its writer lock. Environment settings apply when a new daemon starts; an existing shared daemon keeps its original
environment. `--no-watch` may be added to the server args to disable automatic index synchronization. If there
was no index when the member booted, initialize it yourself and then pause/resume the member (or restart the
daemon) to refresh discovery. `teamagents mcp list` checks discovery without a model, and
`cargo run --offline --locked --manifest-path engine/Cargo.toml --example codegraph_probe -- /absolute/project`
checks discovery plus a real codemode query against an existing index. Stale-source warnings still require direct
file verification, and graph results never replace tests.

## 6. Recovery, compaction and cleanup

- Crash recovery uses persisted facts: verified results are reused, lost outcomes become `OUTCOME_UNKNOWN`,
  and side effects are not replayed on a guess. A full disk stops new side-effect dispatch and parks the
  affected instance with the failure recorded.
- Compaction uses actual context-window usage. Summaries retain the request, revisions, acceptance criteria
  and open questions; full text remains available through `read_history`. Compaction calls count against the
  goal budget.
- `history_days` is applied at daemon boot and `archived_days` by `teamagents sessions prune`. Defaults keep
  history and archived sessions. Explicit retention and deletion can remove data, so preserve what you need
  before changing them. Incompatible database formats are rejected rather than silently adopted.
- `teamagents artifacts` lists stored objects; `teamagents artifacts gc` collects unreferenced ones while
  holding the coordinator lock and refusing a live daemon. Completed-owner release requires a nonzero
  `history_days` window. Live/unknown work and remaining references stay protected; this is not a blanket
  workspace or external-state rollback.

## 7. Troubleshooting

Run `teamagents instances`, `teamagents tasks` or `teamagents status --json` against the same session
(`--state-root` or `--session`). The TUI status highlights attention states, and `/help` shows bounded
diagnostics with recovery instructions. These reads do not send prompts or change state.

Diagnostics distinguish idle-with-unsettled-work from active model/tool/check work, approval and capability
waits, paused/parked instances and registered waits. `READY` with queued input is not idle. A timer-free wait
or a long-running command is not automatically a deadlock or stall; the report is a snapshot, not a promise
about the next command.

Use the instance/task ids in the suggested commands. For an idle worker, send a new instruction or cancel its
unneeded task. `BLOCKED` does not satisfy a task wait. To stop an executing command, use confirmed instance
termination, not task cancellation or model interruption. Inspect unknown effects before retrying. A
`CANCELLED` task does not prove its command stopped. Missing historical reasons remain unavailable, and no
automatic reminder is sent.

| Symptom | What to do |
|---|---|
| `exec: connect ... Connection refused` | Run `teamagents doctor` to inspect the state root; the next `teamagents`/`exec` starts the daemon automatically |
| `the daemon exited while starting (exit status: 1): …` | The reason is the daemon's own first words; the full log is `<state root>/daemon.log` (usually a missing/broken config or an unset credential) |
| `exec: the leader instance i-leader is PARKED …` | The leader stopped on a permanent failure and nothing drives it. The client reports the park as soon as it sees it, with the runtime's own reason (D-164; the same sentence is the TUI's system note and a `daemon.log` line): exit 1 when the input was already submitted (it is waiting — resuming the instance runs it) or exit 2 with "nothing was submitted" when the park was visible before submitting. Fix the cause, then `teamagents instances resume --id i-leader`, or start a fresh state root. A **TERMINATED** leader is different: termination is final, so `exec` says so and the only way on is a fresh state root (D-82) |
| `exec` reports `check 1: FAILED` | Your own `--check` command failed; its output is on stderr and in `<state root>/verification.json` |
| `exec` exits 3 | An operation needs approval and a headless run cannot answer it. Decide it in the TUI or with `teamagents approvals`, then run `exec` again. The compatibility `--full-auto` flag does not bypass self-update approval |
| `doctor` reports the state root as FAIL | That path does not hold a current session database, and the row says which: a path that is a file where a directory belongs (D-166), a file that is not a database, or a format/version stamp mismatch. Use another `--state-root` or follow the message, and never edit the database by hand |
| The agent worked in the wrong directory | Its session was started with another workspace (or without `--cwd`): the client prints the live one. Stop that daemon (§1: `teamagents daemon --stop`, which needs no pid) or start a fresh `--state-root` with `--cwd DIR` |
| The model returns 401/402 | Check the environment variable named by the profile's `api_key_env`; `doctor` lists the credential resolution result per profile |
| A task stays `RUNNING` while its assignee is idle | The assignee's model ended its turn without settling it (D-65): cancel the task — `teamagents tasks cancel --id` — which releases the delegator's wait |
| A worktree member finished and I want its work in my tree | `teamagents instances merge --id <member>` (D-252). It refuses while the member has a turn in flight or its checkout has uncommitted changes, and reports a conflicting merge with git's message, leaving the merge in progress |
| A member is stuck in a long or endless command and cancelling its task changed nothing | `tasks cancel` is delegation-level and does not touch the assignee's operation (D-88). Stop the work with `teamagents instances terminate --id … --yes` (the process group dies within seconds, and the receipt says `class: cancelled`) or wait for the command's own tool timeout. If it is the *thinking* you want to stop rather than a running command, `teamagents instances interrupt --id …` cancels just the current turn (D-363) |
| An instance is parked | `teamagents instances` shows which **and why** (the reason rides on the row, D-165); resume it with `instances resume --id` once that cause is gone |
| An operation approval is pending | Decide it with `teamagents approvals` (§4.1) or in the TUI approvals panel; ordinary Shell creates no approval in new production sessions |
| A command left a service running (`dev-server &`) | By design the session does not manage it (D-41/D-112): it survives the client, the daemon and the member. Stop it yourself with the pid the command printed (`sleep 300 & echo $!`, then `kill <pid>`), or start such work in a command that exits when you are done. `instances terminate` stops an *operation that is still running*, not a service left behind by one that finished |
| A member says it cannot run shell commands although you granted it | Turn the offer into a witness: start the session with `TEAMAGENTS_LOG_SURFACE=1` and the driver writes one line per request into `<state root>/daemon.log` — `driver: surface <instance> shell=yes\|no tools=…`. `shell=no` after a live `authority grant` is a bug worth reporting; `shell=yes` with no attempt means the model had the tool and did not use it (§3.1, D-143/D-157) |
| `exec: the daemon's socket was lost …` | The daemon stopped while this run was waiting — by your own `kill` (§1) or because it crashed. The session and its committed state are kept: start it again (`teamagents` or `exec`) and the work is where it was (D-150) |
| A `teamagents jobs-runner` process is still in `ps` | By design a runner outlives the session that started it: it is the live partner for a job whose outcome is unknown (§6.2). It costs no measurable CPU while it waits (D-153) and exits by itself once its job directory is gone; opening the session again (`teamagents`/`exec` on that state root) imports the receipt and retires it (D-112) |
| A state root holds a lot of bytes and its session is gone | `teamagents artifacts` lists what the catalog holds — kind, size, completeness, the fact that owns each artifact and whether its bytes are on disk — and `teamagents artifacts gc` collects the ones nothing references (a `DELETING` row a crashed sweep left behind), taking the coordinator lock so it refuses while a session is live. It reads what the root really holds (the ordinary store open; a root this process cannot write is read read-only when its log can be attached and refused by name when it cannot), never a silent "nothing here". **Since D-436 it frees more than the recovery half**: an artifact's *owning* fact stops holding it once it has finished — a request that is not `PENDING`, an operation whose status is terminal but not `OUTCOME_UNKNOWN` — and the release is gated on `[retention] history_days`, so with the default 0 nothing is released and the sweep behaves exactly as before. What a reference still points at (the conversation's own entry) stays; whether *that* may expire is the rest of D-253's policy question and is still the user's |
| A state root still runs a `jobs-runner` (`ps -eo pid,args \| grep "[t]eamagents jobs-runner"`) | `teamagents runners` lists every job the state root carries — its journal state, whether a runner still answers and whether the command's child is still alive — and `teamagents runners stop` asks each runner to retire (the token in the job directory is the address, so no pid is guessed). A runner whose command is *still running* refuses, so nothing in flight is taken away; that one is the design's live partner (§6.2/§6.3) and ends when its command does, or is cancelled with `instances terminate` (D-88). A runner from a build this one cannot talk to is reported `unreachable`: retire it with one `kill <pid>` per process, never by pattern (D-148/D-189) **And a command that started a service and did not wait for it** (`dev-server &`) leaves the whole process group behind: `runners stop --service --yes [--id JOB]` signals that group (the row's `service` column is the count). It refuses two things by construction: a job that is not settled (a running command is work, not a leftover) and a group that is not the job's own — after the direct child is reaped the kernel may hand its pid out again, so the identity is *when the group's members were born*: every member must predate the job's own end (D-251). A service that daemonized itself (`setsid`) leaves this group and is out of reach by design |
| Start completely fresh | Stop the daemon (§1: `teamagents daemon --stop`) and run `teamagents --state-root <new directory>` for a clean session; the old database stays where it is |
