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
  re-checked at dispatch; limited delegation, cascading revocation, timeouts and unknown outcomes all have
  recovery classifications.
- **Workspace policies**: `spawn` can give an instance the shared project directory, a private isolated
  directory or its own Git worktree and branch. Termination retires the workspace from its record; a
  directory with uncommitted or unmerged work is never deleted automatically, only reported.
- **Mixed models**: real instances speaking the three wire protocols (responses / anthropic /
  chat-completions) can share one session, each with its own model, effort and native context window.
- **Single source of truth**: one SQLite database per session (WAL with `synchronous=FULL`). Crash recovery
  is classified by persisted location — known results are reused, in-flight losses are recorded honestly
  (`OUTCOME_UNKNOWN`), and nothing is **replayed on a guess**.
- **Permission gate**: `approved_scope` (default: bubblewrap isolation, approvals for out-of-scope work) and
  `full_auto` (user-only, host shell); approvals bind a concrete operation and its argument hash, and `once`
  expires after use.
- **Completion gate**: `finish` only accepts honest outcomes; required checks defined by the user or project
  must actually pass.
- **Long-context compaction**: triggered by real window usage; the summary keeps the original request, user
  revisions, acceptance criteria and open questions, while the full text stays retrievable through
  `read_history`. Compaction calls count against the goal budget.
- **Tools**: file read/write/search with atomic multi-file edits, a persistent in-session shell (`cd` and
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
teamagents doctor                     # config / credentials / state root / isolation probes
teamagents --cwd /path/to/project     # your project directory; omit --cwd to use the current one
```

`init` follows the XDG layout; the default model is DeepSeek Flash (1M context) and the generated TOML can be
edited for anything else. Upgrading from v0.1.1 copies a config template, so `init` can be skipped.

Then just state your goal:

```
Make the tests under /tmp/proj pass and explain why each file changed.
```

The Leader decides whether to split the work and who does what. It only interrupts you for approvals
(out-of-scope commands, network access). Press `Tab` to switch instances, `F3` for the instances panel, `F4`
for tasks and `F5` for the topology.

For scripts and CI, use the headless entry point (the prompt may come from stdin with `-`; `exec` shares the
daemon with the TUI):

```bash
teamagents exec "What is 1+1? Answer directly."                  # one input, print the reply
teamagents exec --json --timeout 180 "Make /tmp/proj tests pass"  # machine-readable summary
teamagents exec --check "cargo test --offline" "Get the tests green"  # extra artifact check, same permissions
```

## Common arguments and keys

| Usage | Meaning |
|---|---|
| `--cwd DIR` | work in DIR (default: the current directory) |
| `--state-root PATH` | use a specific state root (default `$XDG_STATE_HOME/teamagents/v2`) |
| `--model KEY` | pick a model catalog entry (default `leader_main`) |
| `--full-auto` | user-only full-auto mode (out-of-scope work is approved instead of requested) |
| `init` / `doctor` / `daemon` / `exec` / `version` / `--help` | config and state root / self-check / run the daemon alone / headless input / version / usage |

TUI keys (they match the hint line at the bottom): `Enter` send, `Shift+Enter`/`Ctrl+J` newline, `Tab`
switch the input focus and instance, `F1` conversation, `F3` instances, `F4` tasks, `F5` topology, `F2`
approvals, `Esc` back, `Ctrl+C`/`Ctrl+D` quit. In the instances panel `Enter` sets the conversation target,
`p` pauses, `r` resumes and `t` terminates (with confirmation); in the tasks panel `c` cancels a task; in the
approvals panel `a` approves once and `d` denies.

## Configuration and team

- The user config `$XDG_CONFIG_HOME/teamagents/config.toml` is read first, then the project config
  `<cwd>/.teamagents/config.toml` (the user config wins on conflicts; project tool bindings need
  `[permissions] trust_project_tools = true`).
- A model profile selects its wire format with `protocol = "responses" | "anthropic" | "openai" |
  "deepseek"`; `base_url` plus `model` decide the actual service, and credentials are only referenced by
  environment-variable name.
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

中文文档：[README.zh-CN.md](README.zh-CN.md)
