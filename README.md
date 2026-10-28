# TeamAgents

TeamAgents is a Linux terminal coding agent built around a Leader and optional worker instances. You describe
the goal; the Leader decides whether to work alone or delegate parts of it. A daemon keeps the session running
when you close the TUI, and each session stores its authoritative state in one SQLite database.

- The TUI and `teamagents exec` are clients of the same daemon and reconnect from the last event watermark.
- Tasks, grants, budgets, approvals, receipts and events are persisted together. Recovery reuses verified
  results and records unknown outcomes instead of repeating side effects on a guess.
- Instances can use different models, tools and workspaces. The control plane checks grants when work is
  dispatched, including work that was queued before a grant was revoked.

## Documentation

| Document | Contents |
|---|---|
| [User guide](docs/USER-GUIDE.md) | Commands, configuration, permissions, Skills/MCP, recovery and cleanup |
| [Daily coding readiness and comparison](docs/PRODUCT-COMPARISON.md) | Current Pi comparison, evidence limits and candidate work |
| [Acceptance](docs/ACCEPTANCE.md) | Evidence for A01–A36 and known gaps |
| [Capabilities](docs/CAPABILITIES.md) | Capabilities, their checks and the limits of those checks |
| [Design baseline](docs/DESIGN.md) | Confirmed requirements, architecture and acceptance criteria |
| [Development](docs/DEVELOPMENT.md) | Toolchain, test gates and maintenance commands |
| [Decisions](docs/DECISIONS.md) | Confirmed decisions and their history |
| [Install guide](docs/INSTALL.md) | Download, source builds, upgrades and uninstall |
| [Formal verification](verification/README.md) | TLA+ and Kani properties, commands and unproven areas |

<!-- The reading map (D-443): every tracked document declares its reader and exactly one home, the map below
     points at it, and review/doc_inventory.py checks the three directions. The entry points need no line. -->
<!-- maps: docs/ACCEPTANCE.md reader=evidence -->
<!-- maps: docs/CAPABILITIES.md reader=evidence -->
<!-- maps: docs/DECISIONS.md reader=evidence -->
<!-- maps: docs/DESIGN.md reader=developer -->
<!-- maps: docs/INSTALL.md reader=user -->
<!-- maps: docs/PRODUCT-COMPARISON.md reader=user -->
<!-- maps: docs/USER-GUIDE.md reader=user -->

New here? Start with the [install guide](docs/INSTALL.md). The published release is **v0.2.0** for Linux x86_64.

## How it works

```text
you ──goal──▶ Leader ──spawn / delegate──▶ worker instances
                │                               │
                │                               ├── shared files, worktrees and messages
                │                               └── task results
                ├── capability requests and self-update approval ──▶ you
                └── finish only after the required checks pass
```

The Leader can work alone. When it creates a team, collaboration goes through the control plane rather than
private worker-to-worker channels. The TUI stays focused on the Leader; the CLI provides member, task, goal
and session management.

## Highlights

- **Delegation with explicit authority**: the Leader receives session-level `manage`, `delegate` and `message`
  grants at startup. Workers do not receive Shell automatically. You can grant it directly or issue a
  delegable parent grant for task-scoped children. Children cannot widen or redelegate that authority.
- **Shared or separate workspaces**: a worker can use the project directory, a private directory or its own
  Git worktree. TeamAgents does not delete worktrees with uncommitted or unmerged work.
- **Multiple model protocols**: Chat Completions, DeepSeek, Responses and Anthropic profiles can share a
  session. Model, effort and native context window are configured per profile.
- **Host execution**: Shell, acceptance commands and local MCP services use the host filesystem and network.
  There is no built-in OS sandbox or per-command network policy. File tools still enforce workspace paths;
  an authorized Shell can access whatever the host user can access.
- **Coding tools**: file reads, searches, writes and revision-checked edits; a persistent per-member Shell;
  DeepSeek/Exa web search and DeepSeek fetch; Skills; and MCP over stdio or streamable HTTP. The `codemode` tool
  composes bound MCP calls in QuickJS without exposing direct file or network capabilities to the script.
- **Completion checks**: required `[[checks]]` must pass before a goal can settle successfully. Separate
  `exec --check` commands validate a finished turn and control the client's exit code.
- **Recovery and retention**: durable runner journals support reconnect after a daemon restart. Failed or
  interrupted work is classified from persisted facts. History retention and artifact collection are explicit
  policies, not a general workspace rollback mechanism.
- **Terminal interface**: Markdown and syntax-colored code, diagrams and local image previews, attachments,
  pending-input editing, session search, and local Markdown/JSON export. See the
  [maturity report](review/maturity-2026-10-11/REPORT.md) for its pinned local evidence and limits.

## Install

TeamAgents supports Linux x86_64. Model credentials come from environment variables and are not written to
the generated config.

### Install the published release

The v0.2.0 archive is statically linked against musl, so it does not need a Rust toolchain at runtime:

```bash
(
  set -eu
  installer="$(mktemp)"
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://raw.githubusercontent.com/seek-hope/TeamAgents/main/install.sh -o "$installer"
  sh "$installer"
)
export PATH="$HOME/.local/bin:$PATH"
teamagents init
```

The installer selects the latest release, verifies `SHA256SUMS`, and installs `teamagents` and
`teamagents-tui` together. Repeating it upgrades both binaries without replacing your config or sessions.
See the [install guide](docs/INSTALL.md) for `--version`, `--bin-dir` and local archives. Source builds may
contain changes that are not in v0.2.0.

### Building from source

```bash
cargo build --locked --release --manifest-path engine/Cargo.toml --bin teamagents
cargo build --locked --release --manifest-path tui/Cargo.toml --bin teamagents-tui
mkdir -p ~/.local/bin
install -m755 engine/target/release/teamagents tui/target/release/teamagents-tui ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

Use the toolchain pinned in `rust-toolchain.toml`. Once dependencies are cached, add `--offline`.

## Quick start

```bash
teamagents init                       # create a minimal config and state root; preserve existing config
export DEEPSEEK_API_KEY='your key'    # the default profile reads this variable
teamagents doctor                     # check config, credentials, state, Skills and host execution
teamagents --cwd /path/to/project     # omit --cwd to use the current directory
```

`init` writes an XDG-compliant config for DeepSeek Flash with its native 1M context and high reasoning effort.
It never merges new settings into an existing file; edit that file deliberately and inspect it with `doctor`.

Then describe the result you want:

```text
Make the tests under /tmp/proj pass and explain why each file changed.
```

The Leader chooses whether to delegate. Ordinary Shell calls run automatically on the host; workers still
need the appropriate capability grant. Self-update requires a separate user approval.

For scripts and CI, use `exec`. It shares the daemon with the TUI, and `-` reads the prompt from stdin:

```bash
teamagents exec "What is 1+1? Answer directly."
teamagents exec --json --timeout 180 "Make /tmp/proj tests pass"
teamagents exec --stream-json "Get the tests green" | jq -c '.event.kind'
teamagents exec --check "cargo test --offline" "Get the tests green"
teamagents exec --accept tests="pytest -q" "Fix the failing tests"
git diff | teamagents exec -
```

`--stream-json` emits one event object per line, followed by the same report object that `--json` prints.
Closing the consumer ends the stream, not the underlying run.

Exit codes are `0` for a completed goal or direct answer, `1` for failure or an unfinished run, `2` for usage
or infrastructure errors, `3` when an operation needs approval, and `124` when `--timeout` expires.

`--check COMMAND` runs after a finished turn and stops at the first failure. `--accept ID=COMMAND` attaches a
required check to the goal before the input is submitted, so a failure can trigger bounded repair instead of
merely changing the exit code. See [goal limits](docs/USER-GUIDE.md#22-goal-limits-limits) for token, deadline
and per-tool limits.

Manage capabilities through the same session:

```bash
teamagents authority
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority grant --subject i-leader --action shell --scope workspace --delegable
teamagents authority revoke --grant g-1a2b3c4d
```

Every dispatch checks the current grants. Revoking a parent also revokes its children; task-scoped grants
expire with their task or instance.

## Common arguments and keys

| Usage | Meaning |
|---|---|
| `--cwd DIR` | Workspace for a new session. A running daemon keeps its existing workspace |
| `--state-root PATH` | State base, default `$XDG_STATE_HOME/teamagents/v2`; named sessions live below it |
| `--session ID` | Attach to a named session registered under that base |
| `--model KEY` | Select a model profile, default `leader_main` |
| `--every MINUTES` / `--prompt TEXT` | Set an automation's period and input |
| `--query TEXT` / `--limit N` | Set a session-search query and result limit |
| `--full-auto` | Compatibility flag; new sessions already use automatic host Shell |

Use `teamagents --help` for the complete command list. The main groups are `init`, `doctor`, `daemon`, `exec`,
`authority`, `approvals`, `instances`, `tasks`, `goals`, `runners`, `artifacts`, `sessions`, `automations`,
`mcp`, `status`, `history`, `surface` and `version`. `daemon --stop` stops the session that owns the selected
state root; closing the TUI does not.

Common TUI keys:

- `Enter` sends; `Ctrl+J` or `Shift+Enter` inserts a newline. `↑`/`↓` recall input or move within a draft;
  `PageUp`/`PageDown` and the mouse wheel scroll. `Ctrl+W` deletes a word and `Ctrl+←`/`Ctrl+→` move by word.
- `Ctrl+A` focuses approvals (`a` approves, `d` denies, `Esc` returns). `Ctrl+C` interrupts the current model
  request, not running tools or delegated work. `Ctrl+Q` or `/quit` closes the TUI without stopping the daemon.
- `Ctrl+M` or `/model` opens the Leader's model/effort menu. `Ctrl+P` or `/queue` opens pending input.
  `Ctrl+S` or `/sessions` opens session history and switching. `Ctrl+E` edits the draft in `VISUAL` or `EDITOR`.
- `Ctrl+O` expands or folds tool previews. `Ctrl+G` switches between previews and source.
  `TEAMAGENTS_GRAPHICS=auto|kitty|blocks|off` selects the graphics mode.

Slash commands are `/help`, `/sessions`, `/interrupt`, `/quit`, `/model`, `/attach`, `/detach`, `/queue` and
`/edit`. Completion fills a command without sending it; press Enter separately to submit.

For stored state, use `teamagents status --json` or
`teamagents history --id INSTANCE --limit 400 --json`. The [user guide](docs/USER-GUIDE.md#1-quick-start)
documents the remaining management commands and report fields.

## Configuration and team

- The user config is `$XDG_CONFIG_HOME/teamagents/config.toml`. TeamAgents also searches upward for
  `.teamagents/config.toml`, but project settings have no effect until you trust the canonical project root
  with `trusted_projects` or set `trust_project = true`. Your definitions win on name conflicts.
- Trusted projects can contribute model profiles, tool bindings, Skill paths and instruction files. They
  cannot contribute permissions, hooks, checks, retention, limits or update policy. Trusted `AGENTS.md` files
  are discovered from the repository boundary to the working directory; untrusted projects contribute none.
- A model profile selects `protocol = "responses" | "anthropic" | "openai" | "deepseek"`, an endpoint, a
  model and a credential variable. Use the model's native context window rather than an invented smaller one.
- Required `[[checks]]` belong only to the user config and run on the host. The Leader creates teams at runtime;
  there is no static team-definition file.

The [configuration section](docs/USER-GUIDE.md#2-configuration) covers precedence, limits and hooks.

## Status and limits

- Linux x86_64 is the supported build target. Windows, macOS, distributed workers and a general extension API
  are not supported product surfaces.
- Host Shell and local MCP have the host user's access. Remove retired sandbox settings explicitly when
  upgrading an old config; TeamAgents does not delete existing sessions, VMs or system packages for you.
- TeamAgents is suitable for controlled coding trials, but current evidence does not establish it as a
  drop-in replacement for Pi across general development work. The comparison and remaining candidates are in
  [PRODUCT-COMPARISON.md](docs/PRODUCT-COMPARISON.md).
- `engine/tests/providers_fake.rs` covers all four protocol families locally. Live provider, OAuth and image
  behavior depends on the service and credentials used; local tests are not live-service acceptance.
- [Acceptance evidence](docs/ACCEPTANCE.md) records what was tested, against which source, and what remains
  open. Historical benchmark results do not automatically apply to later source revisions.

## Development

Run `make check` before committing and `make pty` for terminal checks. See
[DEVELOPMENT.md](docs/DEVELOPMENT.md) for the complete workflow and [AGENTS.md](AGENTS.md) for repository
conventions. Review evidence lives in `review/`; formal-verification material lives in `verification/`.

A `v*` tag builds a release archive, but publication also requires the `TEAMAGENTS_RELEASE_APPROVED`
repository variable and explicit release authorization. The existing v0.2.0 release is not replaced by
ordinary source changes.

Chinese documentation: [README.zh-CN.md](README.zh-CN.md)
