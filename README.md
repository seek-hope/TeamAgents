# TeamAgents

**You talk to the Leader, and the Leader builds the team on the spot.** A team-style agent product for the
Linux terminal: you state a goal, the Leader recruits working instances, delegates tasks, coordinates them
and reports back; you watch progress and approve anything that leaves the sandbox.

- **Single source of truth**: one SQLite database per session (WAL with `synchronous=FULL`). Instances,
  tasks, grants, budgets, approvals, receipts and events commit in one transaction; crash recovery is
  classified by persisted location and never replays on a guess.
- **A daemon owns the session**: the TUI and `exec` are thin clients of it (JSON over a Unix socket; a
  reconnect resumes events from the last watermark).
- **Governed collaboration**: `spawn` / `delegate` / `send` / `wait` all go through control-plane
  authorization and re-check the permission revision at dispatch.
- **Multiple providers**: one session can mix real instances speaking DeepSeek (chat-completions),
  Responses and Anthropic.

## Documentation

| Document | Contents |
|---|---|
| [User guide](docs/USER-GUIDE.md) | Getting started, configuration, permissions, Skills/MCP, recovery and cleanup |
| [Comparison with Codex CLI, Pi and Hermes](docs/PRODUCT-COMPARISON.md) | a dated, sourced snapshot of how this product compares, and the decisions it surfaces |
| [Acceptance](docs/ACCEPTANCE.md) | Per-item evidence for the A01–A36 acceptance matrix, plus known gaps |
| [Design baseline](docs/DESIGN.md) | Confirmed requirements, architecture and protocol constraints, A01–A36, definition of done |
| [Development](docs/DEVELOPMENT.md) | Toolchain, gates, layout conventions, re-run commands |
| [Decisions](docs/DECISIONS.md) | Confirmed direction, boundaries and deviations |
| [Install guide](docs/INSTALL.md) | Download, install and upgrade |
| [Formal verification](verification/README.md) | TLA+ specs and Kani proofs: properties ↔ code ↔ acceptance items, unproven list |

> **First time here:** [download and install](docs/INSTALL.md) · [latest release](https://github.com/seek-hope/TeamAgents/releases/latest)

## How it works

```
you ──goal (natural language)──▶ Leader ──spawn / delegate──▶ instances A / B / C (in parallel)
                                   │  ▲                            │
                                   │  └────────── send ────────────┘  instances never talk
                                   │                    privately; collaboration goes through
                                   │                    control-plane-granted messages and shared space
                                   ├── permission / approval request ──▶ you (only for out-of-scope work)
                                   └── finish: the goal is done when the Leader passes the completion checks
```

- You **always** talk to the Leader and never direct instances yourself; whether to split work and how many
  instances to use is the Leader's call (a team of one is legal).
- Every instance has its own model, tool bindings, workspace policy and budget. Work that exceeds its scope
  becomes an **approval request** while the rest of the team keeps working.
- Runtime facts (who is doing what, where a task is stuck, why a turn ended) all land in the session
  database: queryable, recoverable, auditable.

## Highlights

- **Governed collaboration**: `spawn` / `delegate` / `send` / `wait` are authorized by the control plane and
  re-checked at dispatch; the session grants the Leader its team authority by default (`manage` / `delegate` /
  `message` over the session), and a grant can be revoked at any time — revocation cascades, and limited
  delegation, timeouts and unknown outcomes all have recovery classifications. The tool surface follows the
  grants, so an instance is offered only what it can dispatch — a worker the Leader spawned holds no shell until
  you grant it `shell@workspace` (D-60).
- **Workspace policies**: `spawn` can give an instance the shared project directory, a private isolated
  directory or its own Git worktree and branch. Termination retires the workspace from its record; a
  directory with uncommitted or unmerged work is never deleted automatically, only reported. A worktree's
  branch is yours to merge (name and base commit in `<state root>/instances/<id>/worktree.json`); once it is
  merged, the running session retires the checkout by itself.
- **Mixed models**: real instances speaking the three wire protocols (responses / anthropic /
  chat-completions) can share one session, each with its own model, effort and native context window; `spawn`
  takes the catalog entry for a worker (key or model name), so one team can mix entries. An instance whose
  entry is unknown is refused as that tool call's error, and one that can never boot is parked with the reason
  instead of stopping the session.
- **Single source of truth**: one SQLite database per session (WAL with `synchronous=FULL`). Crash recovery
  is classified by persisted location — known results are reused, in-flight losses are recorded honestly
  (`OUTCOME_UNKNOWN`), and nothing is **replayed on a guess**.
- **Permission gate**: `approved_scope` (default: bubblewrap isolation, approvals for out-of-scope work) and
  `full_auto` (user-only, host shell); an approval binds one concrete operation, its argument hash and its
  permission revision, so it is used up by that single dispatch.
- **Completion gate**: `finish` only accepts honest outcomes; required checks defined by the user or project
  must actually pass.
- **Long-context compaction**: triggered by real window usage; the summary keeps the original request, user
  revisions, acceptance criteria and open questions, while the full text stays retrievable through
  `read_history`. Compaction calls count against the goal budget.
- **Tools**: file read/write/search with exact-match edits, SHA-256 version checks and atomic writes (a
  process-wide write lock serializes mutations), a persistent in-session shell (`cd` and
  `export` survive across commands), web search and fetch, MCP (stdio and streamable HTTP) and Skills
  (`~/.agents/skills`).
- **User hooks (`[hooks]`)**: `notify` forwards events (`tool_call` / `team_action` / `run_*`) to your own
  program; `pre_tool` can veto any native tool call before it runs (exit 2 denies, stderr is the reason),
  and a broken hook only logs instead of stalling the team.
- **Isolation**: under `approved_scope` instance shells run in `bubblewrap`; when it is missing the command
  fails loudly instead of silently degrading to unsandboxed execution.

## Install

Requirements: Linux (x86_64) plus `bubblewrap`; model credentials are read from environment variables and
never written into the config.

### Latest release (recommended)

The repository and the [releases](https://github.com/seek-hope/TeamAgents/releases/latest) are public: no
GitHub login and no Rust toolchain needed. The installer picks the latest version, verifies SHA-256 and
installs both binaries into `~/.local/bin`:

```bash
(
  set -eu
  installer="$(mktemp)"
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://raw.githubusercontent.com/seek-hope/TeamAgents/main/install.sh -o "$installer"
  sh "$installer"
)
export PATH="$HOME/.local/bin:$PATH"
```

Add `export PATH="$HOME/.local/bin:$PATH"` to `~/.bashrc` or `~/.zshrc` so later terminals find it. Quit
TeamAgents before upgrading and run the installer again; the existing config and sessions are kept. Pinned
versions, custom install directories and local archives are documented in the [install guide](docs/INSTALL.md).

### Building from source

```bash
cargo build --locked --release --manifest-path engine/Cargo.toml --bin teamagents
cargo build --locked --release --manifest-path tui/Cargo.toml --bin teamagents-tui
mkdir -p ~/.local/bin
install -m755 engine/target/release/teamagents tui/target/release/teamagents-tui ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

This needs a Rust toolchain pinned by `rust-toolchain.toml`; add `--offline` once dependencies are cached.

## Quick start

Install `bubblewrap` (Debian/Ubuntu: `sudo apt install bubblewrap`; other distributions are covered in the
install guide), then run in one terminal:

```bash
teamagents init                       # write a minimal config and prepare the state root; never overwrites
export DEEPSEEK_API_KEY='your key'    # the default config uses DeepSeek; other services follow api_key_env
teamagents doctor                     # config / credentials / state root / skills / isolation probes
teamagents --cwd /path/to/project     # your project directory; omit --cwd to use the current one
```

`init` follows the XDG layout; the default model is DeepSeek Flash (1M context) and the generated TOML can be
edited for anything else. Upgrading from v0.1.1 copies a config template, so `init` can be skipped.

Then just state your goal:

```
Make the tests under /tmp/proj pass and explain why each file changed.
```

The Leader decides whether to split the work and who does what. It only interrupts you for approvals
(out-of-scope commands, network access). Press `Ctrl+N` to cycle the panels (instances, tasks, topology),
`Tab` to switch the conversation target and `Ctrl+A` for pending approvals.

For scripts and CI, use the headless entry point (`exec` shares the daemon with the TUI; `-` reads the
prompt from stdin, so a long instruction can be piped in):

```bash
teamagents exec "What is 1+1? Answer directly."                  # one input, print the reply
teamagents exec --json --timeout 180 "Make /tmp/proj tests pass"  # machine-readable summary
teamagents exec --check "cargo test --offline" "Get the tests green"  # plus your own acceptance check
git diff | teamagents exec -                                      # the prompt comes from stdin
```

Exit codes: `0` settled, `1` failed or unfinished, `3` an approval is pending (a headless run has nobody to
answer it, so it reports instead of waiting), `124` the `--timeout` deadline passed, `2` usage or
infrastructure (no daemon, no model profile). Each `--check COMMAND` runs after the turn ends, in order, in
the isolated shell inside your workspace (`--cwd` or the current directory); the first failure stops the list
and makes the run fail. The verdicts are printed, written to `<state root>/verification.json` and included in
the `--json` report as `verification`.

**Your session, your ceilings.** `[limits]` in the config bounds every goal — `max_total_tokens` refuses a
request that would pass the usage ceiling and `deadline_minutes` refuses one past the deadline; without them a
session runs until you stop it, which `doctor` says out loud. See
[docs/USER-GUIDE.md](docs/USER-GUIDE.md) §2.2.

**Capabilities are yours to hand out.** The Leader gets the authority its own team tools need; a worker it
spawns gets none of it, so it works with the file, web and skill tools until you grant it more — for example
the shared-workspace shell a coding task usually needs:

```bash
teamagents authority                                    # instances and grants, with the ids
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority revoke --grant g-1a2b3c4d          # final; takes derived grants with it
```

Grants are checked again at every dispatch, so a revocation stops work that was already queued, and the tool
leaves the model's surface at its next request. The surface refuses what could never do anything (an unknown
action, or a pair such as `shell@instance:i-worker-1` that no check asks about) and tells you what to use
instead. Details, the full action vocabulary and the exit codes are in
[docs/USER-GUIDE.md](docs/USER-GUIDE.md) §3.1.

## Common arguments and keys

| Usage | Meaning |
|---|---|
| `--cwd DIR` | work in DIR (default: the current directory); it applies to the session this command starts — a running session keeps its own workspace, and the client prints that one instead of working somewhere else. A path that is not an existing directory is refused before anything starts (the session confines every file and shell command to it) |
| `--state-root PATH` | use a specific state root (default `$XDG_STATE_HOME/teamagents/v2`); it is the *directory* holding `session.sqlite` and `daemon.sock`, created when it does not exist yet — a path that is a file is refused with the flag named (`doctor`, `init`, `daemon`, `exec` and the session verbs) |
| `--model KEY` | pick a model catalog entry (default `leader_main`) |
| `--full-auto` | user-only full-auto mode (out-of-scope work is approved instead of requested); it applies to the session this command starts — a session that is already running keeps the mode it booted with, and the client prints that mode instead of pretending |
| `init` / `doctor` / `daemon` / `exec` / `authority` / `approvals` / `instances` / `tasks` / `version` / `--help` | config and state root / self-check (config, credentials, state root, skills, goal limits, checks, hooks, bubblewrap) / run the daemon alone (its output goes to `<state root>/daemon.log`) / headless input / list, grant and revoke capabilities / list and decide approvals / pause, resume, terminate and list instances / list and cancel tasks / version / usage |

TUI keys (they match the hint line at the bottom; deliberately no function keys, since some keyboards lack
them): `Enter` send, `Ctrl+J` newline (and `Shift+Enter` where the terminal reports modifiers), `↑`/`↓` walk a
multi-line draft and recall the prompts you sent (500 deep, the draft comes back when you walk past the
newest), `Ctrl+W` deletes the word before the caret and `Ctrl+←`/`Ctrl+→` jump by word, `PageUp`/`PageDown`
(and the mouse wheel) scroll the conversation, `Tab` switch the
conversation target, `Ctrl+N` cycle the
views (conversation → instances → tasks → topology → back), `Ctrl+A` pending approvals, `Esc` back,
`Ctrl+C`/`Ctrl+D` quit. In the instances panel `Enter` sets the conversation target, `p` pauses, `r` resumes
and `t` terminates (with confirmation); in the tasks panel `c` cancels a task; in the approvals panel
`a` approves once and `d` denies.

## Configuration and team

- The user config `$XDG_CONFIG_HOME/teamagents/config.toml` is what a session reads. A project config
  `<cwd>/.teamagents/config.toml` is **not wired into the daemon yet**: the merge loader and its trust rules
  (`[permissions] trust_project_tools = true` for project tools) exist and are unit-tested, but no entry
  point calls them, so a repository cannot influence a session today (see `docs/ACCEPTANCE.md`).
- A model profile selects its wire format with `protocol = "responses" | "anthropic" | "openai" |
  "deepseek"`; `base_url` plus `model` decide the actual service, and credentials are only referenced by
  environment-variable name.
- `[[checks]]` in the user config (never in a project file) are the machine contracts a goal must satisfy:
  the runtime runs them in the isolated shell at the completion boundary, so a failing check returns the work
  for repair instead of letting a summary claim success.
- The Leader forms the team at runtime (`spawn` / `delegate` / `send` / `wait`); there is no static team
  definition file.
- Tool bindings as authorization, the approval semantics, Skills/MCP, retention and failure handling are in
  the [user guide](docs/USER-GUIDE.md).

## Status and limits

- Linux only; instance shell isolation needs `bubblewrap` and fails loudly when it is missing.
- Releases are x86_64 (musl) only; no Windows/macOS, distributed execution, remote instance protocol or
  browser automation.
- All three wire protocols (responses / anthropic / chat-completions) are implemented with local fake-server
  regression tests; **compatibility acceptance against five real model services still needs environments
  with the corresponding credentials**.
- Acceptance boundaries and evidence live in [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md), which lists every open
  item and known ceiling.

## Development

Run `make check` before anything else; the full workflow is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and
the repository conventions in [AGENTS.md](AGENTS.md). Defects, reviews and evaluation evidence live in
`review/`, formal-verification evidence in `verification/`. Pushing a `v*` tag makes
`.github/workflows/release.yml` build and publish the release archives (with `SHA256SUMS`).

Chinese documentation: [README.zh-CN.md](README.zh-CN.md)
