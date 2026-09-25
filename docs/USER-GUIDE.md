# TeamAgents user guide

This guide describes the current implementation. Notes from earlier releases and the migration material are
archived in [docs/archive/](archive/README.md) and are not the current reference.

## 1. Quick start

```bash
teamagents init                      # write the config (kept if it exists) and prepare the state root
export DEEPSEEK_API_KEY=...          # the environment variable named by api_key_env
teamagents doctor                    # config, credentials, state root, bubblewrap and host checks
teamagents                           # open the TUI (starts the per-user daemon when needed)
```

- **One daemon per user**: `teamagents` probes `$XDG_STATE_HOME/teamagents/v2/daemon.sock` and, when it is
  missing or refuses the connection, starts `teamagents daemon` detached and hands the socket to the TUI.
  Quitting the TUI does not stop the session.
- **Headless use**: `teamagents exec [--json] [--timeout SEC] "prompt"` goes through the same daemon and
  reports the goal's terminal state, the assistant reply or a timeout; exit code 0 means it settled.
- **State root**: `$XDG_STATE_HOME/teamagents/v2/` (default `~/.local/state/teamagents/v2`), where
  `session.sqlite` is the **single source of truth** (WAL with `synchronous=FULL`, carrying a format and
  version stamp).

## 2. Configuration

The user config is `$XDG_CONFIG_HOME/teamagents/config.toml` (default
`~/.config/teamagents/config.toml`). Credentials are referenced by environment-variable name and never
written into the file:

```toml
[models.leader_main]
provider = "deepseek"
protocol = "deepseek"        # deepseek | chat/completions | responses | anthropic
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000     # native window; leave empty when unknown (never guess a small value, D-36)

[tools.web]                  # optional tool binding (web / fetch / mcp)
kind = "web_search"
provider = "anysearch"
url = "https://api.anysearch.com/v1/search"
api_key_env = "ANYSEARCH_API_KEY"
```

- `context_window` drives the request budget and the compaction threshold; real models must use their native
  window, and the value and its source are recorded (D-36 in `docs/DECISIONS.md`).
- `teamagents doctor` checks the config item by item, that every model profile's credential resolves, the
  state root (stamp, WAL, read/write), the bubblewrap isolation probe and that the programs named in
  `[hooks]` are executable. An `sessions/` layout from an earlier release is reported explicitly and is
  never migrated.

### 2.1 Hooks (`[hooks]`)

Hooks are **your own programs** (their paths come only from the user config; a model cannot choose them) and
run on the host with your permissions:

```toml
[hooks]
notify = ["/home/you/bin/teamagent-notify.sh"]    # notifications: argv[1] is the event name, JSON on stdin
pre_tool = ["/home/you/bin/policy.sh"]            # policy hook before tool calls
```

- `notify` runs asynchronously and never blocks a turn; it is killed after 10 seconds and failures only reach
  the engine's stderr. The events are `tool_call` (with `tool`/`arguments`/`ok`/`error`), `team_action`,
  `run_completed`, `run_failed`, `run_cancelled` and `run_paused` (the instance entered PAUSED).
- `pre_tool` runs synchronously before every native tool call (files/Shell/web/Skills/MCP). Exit code 0
  allows it; **exit code 2 denies it** and the first stderr line becomes the reason handed to the model;
  any other exit code, a spawn failure or a timeout allows the call and logs to stderr — a broken hook never
  stalls the team. Replays after crash recovery are not asked again (the decision was made at first
  dispatch), and the required checks in `[checks]` are your own acceptance commands, so they skip `pre_tool`.

## 3. Sessions and teams

- The daemon owns the session: one JSON-lines protocol (`/v1`) over a Unix socket, with the TUI and `exec`
  as thin clients. A reconnect resumes events from the last watermark and commands deduplicate by
  `command_id`.
- **The Leader builds the team**: it uses `spawn` to create working instances, `delegate` to hand out tasks,
  `send` for messages and `wait` for results. These appear in the model-visible tool surface according to
  its grants (`manage`/`delegate`/`message`), and the permission revision is re-checked at dispatch.
- **Workspace policies**: the `workspace` argument of `spawn` decides where a new instance works — `shared`
  (default: the project directory), `isolated` (a private directory under the session state root) or
  `git_worktree` (its own branch and worktree). Asking for a worktree in a project that is not a Git
  repository or has uncommitted changes falls back to shared mode and says why in the tool receipt.
  Terminating an instance retires its workspace from the record, and **a directory with uncommitted or
  unmerged work is never deleted automatically** — only the reason is reported.
- User-side intervention: switch instances, pause/resume/cancel and approve or deny tool requests in the
  TUI. Budget, task and grant panels all read the same facts.
- Goal and task completion goes through the runtime's completion gate: `finish` only accepts honest
  outcomes, and the required checks defined by the user or project must really pass.

## 4. Permissions and isolation

| Mode | Behaviour |
|---|---|
| `approved_scope` (default) | Shell runs under bubblewrap; network access and out-of-scope writes need user approval, bound to the concrete operation and its argument hash |
| `full_auto` | Host shell (D-41): long commands and background services survive across calls, and `exec` exiting does not stop an already started service |

When bubblewrap is unavailable this is a **classified failure** (`started=false`); the command never falls
back silently to host execution.

## 5. Skills and MCP

- Skills live under the registration root `~/.agents/skills` (searched and read on demand with
  `skill search/read`; a skill's instructions can never widen execution permissions).
- MCP: bound services load at startup (a required service fails loudly, an optional one only drops its
  capability). Calls go through the same permission, approval, budget, cancellation and receipt entry
  points. A remote call dispatched before a crash is recorded as `OUTCOME_UNKNOWN` after recovery and is
  **never replayed**.

## 6. Recovery, compaction and cleanup

- Crash recovery is classified by persisted location: known results are reused, in-flight losses are
  recorded honestly (`OUTCOME_UNKNOWN`) and nothing is replayed on a guess. When the disk is full, new
  side-effect dispatch stops and the instance parks, reporting the in-flight loss (A31).
- Long-context compaction triggers on **real window usage**: the summary keeps the original request, user
  revisions, acceptance criteria and open questions, while the full text stays retrievable through
  `read_history`. Compaction calls count against the goal budget.
- Data cleanup: the session state of earlier releases has been removed against an explicit inventory (the
  script and the inventory-based approach are kept in `review/archive/r28-legacy-cleanup.py`; it lists first
  and only deletes with `--apply`). Credentials, `~/.agents/skills`, `~/.codex` and the evidence under
  `review/` are always kept.

## 7. Troubleshooting

| Symptom | What to do |
|---|---|
| `exec: connect ... Connection refused` | Run `teamagents doctor` to inspect the state root; the next `teamagents`/`exec` starts the daemon automatically |
| `doctor` reports the state root as FAIL | That path does not hold a current session database (the stamp does not match); use another `--state-root` or follow the message, and never edit the database by hand |
| The model returns 401/402 | Check the environment variable named by the profile's `api_key_env`; `doctor` lists the credential resolution result per profile |
| A command under `approved_scope` waits for approval | Handle it in the TUI approvals panel, or run with `--full-auto` (host execution, D-41) |
| Start completely fresh | Stop the daemon and run `teamagents --state-root <new directory>` for a clean session; the old database stays where it is |
