# TeamAgents

[Chinese README](README.zh-CN.md) · [User guide](docs/USER-GUIDE.md) · [Design](docs/DESIGN.md) · [Acceptance](docs/ACCEPTANCE.md)

TeamAgents is a Rust-native, local-first agent runtime. A Leader works with the user, can delegate scoped tasks
to other instances, and persists execution, approvals, context, costs and evidence in one authoritative SQLite
store per session. The TUI and headless clients share the same daemon and phase machine.

The current format is **database schema 9**, **socket protocol 1** and **work policy 1**.

## Current capabilities

- Durable work across turns, compaction and restart, with bounded continuation, budgets, deadlines and required
  acceptance checks.
- Scoped collaboration through `spawn`, `delegate`, `send` and `wait`, with shared, isolated or Git-worktree
  workspaces and attenuated capability grants.
- Files, host Shell, web search/fetch, Skills, stdio/HTTP MCP resources and restricted JavaScript `codemode`.
- Chat Completions, DeepSeek, Responses and Anthropic profiles with per-model native context windows and
  configurable reasoning effort.
- One Leader conversation with Markdown/code/media rendering, conditional approvals, pending-input editing,
  session history/search/switch/delete and local attachments.
- One **session-level exploration tree** beside compaction, with revisioned nodes, source references and focus.
- Current executable development and verification tools under `tools/`.

Host Shell, acceptance commands and local MCP are not an OS sandbox. Logical grants and private-history rules
still apply. Unknown effects are recorded as `OUTCOME_UNKNOWN` and never replayed on a guess.

## Build and start

Install the pinned Rust toolchain from `rust-toolchain.toml`, then:

```bash
make check CARGO_FLAGS=--locked   # first run may download dependencies
make build
engine/target/debug/teamagents init
export DEEPSEEK_API_KEY=...       # the environment variable named by your config
engine/target/debug/teamagents doctor
engine/target/debug/teamagents
```

`teamagents` starts or joins the session daemon and opens the TUI. `init` never overwrites an existing config;
the default config path is `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`.
Use `teamagents exec` for a headless turn:

```bash
engine/target/debug/teamagents exec --json --timeout 180 "Inspect this repository and summarize the current state"
```

The default model profile key is `leader_main`; `--model` selects another configured catalog entry.
See [installation and upgrades](docs/INSTALL.md) and [configuration](docs/CONFIG.md) for custom providers,
permissions and storage locations.

## Session exploration tree

The exploration tree records questions, hypotheses, attempts, findings, decisions, obstacles and next steps as a
model-maintained navigation view. It is not verified fact, completion evidence, authority or a way to rewind
side effects.

- One tree per session, persisted with append-only revisions and a head pointer.
- Leader compaction advances the previous cumulative tree through the retained recent turn.
- Member-private history is not copied into the shared tree.
- Nodes carry source references and freshness is compared using the full `(instance, epoch, entry)` watermark.
- `add`, `update`, `move`, `merge` and `focus` use revisioned atomic patches. Ordered operations roll back as a
  unit and replay without creating duplicate revisions.
- Invalid compaction patches leave the old tree stale without blocking the summary or current work.

In the TUI, type `/tree`. In a terminal, run:

```bash
engine/target/debug/teamagents search_tree
engine/target/debug/teamagents search_tree --json
```

Ask the Leader in natural language to merge duplicate branches, abandon a dead end, reorganize the tree or focus
a particular branch. The complete stored history remains separate and readable.

## Everyday operation

Common TUI keys:

- `Enter` sends; `Shift+Enter` inserts a newline. `Up`/`Down` recalls input; `PageUp`/`PageDown` and the wheel
  scroll. `Ctrl+W` deletes a word and `Ctrl+Left`/`Ctrl+Right` moves by word.
- `Ctrl+A` focuses approvals (`a` approves, `d` denies, `Esc` returns). `Ctrl+C` interrupts the current model
  request, not running tools or delegated work. `Ctrl+Q` or `/quit` detaches without stopping the daemon.
- `Ctrl+M` or `/model` opens model/effort selection. `Ctrl+P` or `/queue` opens pending input.
  `Ctrl+S` or `/sessions` opens session history and switching. `Ctrl+E` opens the external editor.
- `Ctrl+O` expands or folds tool previews. `Ctrl+G` switches previews and source.

Slash commands are `/help`, `/sessions`, `/interrupt`, `/quit`, `/model`, `/attach`, `/detach`, `/queue`, `/tree`
and `/edit`. Completion fills a command but submits only after a separate Enter. For file references, type
`@PATH` and Tab, then accept a candidate; files are sent only with your message.

The default state root is `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents/v2`.

Useful CLI commands:

```bash
teamagents status --json
teamagents history --id INSTANCE --limit 400 --json
teamagents search_tree --json
teamagents instances
teamagents tasks
teamagents work_scopes
teamagents sessions
teamagents mcp list
teamagents version
```

Use the same `--state-root` or `--session` when reconnecting to a session. Cancelling a task does not prove that
its process stopped; instance termination and shared-workspace takeover use separate stop evidence.

## Architecture

```text
TUI / exec / CLI
       |
       v
daemon + supervisor + per-instance drivers
       |
       v
bounded storage worker -> Control::submit -> SQLite
       |
       +--> model providers
       +--> tools / Shell runners / MCP
       +--> summaries and session exploration tree
```

The control plane owns identity, permission revisions and atomic team transactions. The engine owns I/O and the
phase machine. SQLite owns authoritative facts with WAL and `synchronous=FULL`. Model and tool waits happen
outside transactions. The TUI owns presentation only.

See [the architecture guide](docs/ARCHITECTURE.md) for state machines and data relationships.

## Current evidence

The current local gate has **777 Rust tests** (core 191, engine 481, TUI 105) and `make check` exit 0.
Formal material contains **393 configurations across 48 modules**:

- **65 positive configurations in 62 traversals**;
- **327 negative controls** refuted;
- **79 normal-path witnesses** reached;
- **9 Kani harnesses** passed.

Search-tree verification includes ordered pair transitions, 23 controls covering every non-`TypeOK` tree claim
and 11 witnesses. Witnesses demonstrate reachability, not safety. PTY covers five terminal profiles and both
forced graphics modes.

Reproduce with:

```bash
make check
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
make pty
```

## Current limits

These results do not establish full Rust/SQLite refinement, whole-system composition, wide exhaustive
completion or Policy B source-bound equivalence. Live probes (`make probe-offline`, `make probe-models`)
exercise the four wire protocols, heterogeneous providers, search/fetch, images, reviewer hardening,
compaction/search-tree updates, isolated clean-host upgrade/rollback, loopback SSH and Alacritty locally;
broader external OAuth services, public hosts, multi-distribution clean hosts, wider terminal matrices,
large-scale long-horizon benchmarks and full priced-cost comparisons remain open. The separate wide
configuration is not claimed complete.

## Documentation

| Document | Contents |
|---|---|
| [User guide](docs/USER-GUIDE.md) | Operation, shortcuts, collaboration, tools and recovery |
| [Design](docs/DESIGN.md) | Requirements and architecture constraints |
| [Acceptance](docs/ACCEPTANCE.md) | Q1–Q19 and A01–A36 evidence/open obligations |
| [Current decisions](docs/DECISIONS.md) | Rules currently in force |
| [Development](docs/DEVELOPMENT.md) | Build, test, formal checks and maintenance |
| [Architecture](docs/ARCHITECTURE.md) | State machines, data and recovery diagrams |
| [Capabilities](docs/CAPABILITIES.md) | Current capability/evidence matrix |
| [Verification](verification/REPORT.md) | Formal results and abstraction limits |
| [Verification guide](verification/README.md) | Current formal material and module index |
| [Product comparison](docs/PRODUCT-COMPARISON.md) | Current open work and comparison obligations |
| [Tools](tools/README.md) | Current development and verification utilities |

Code, documentation and commit messages are English, except `README.zh-CN.md`. Never put credentials in source,
prompts, events or logs.

<!-- maps: docs/USER-GUIDE.md reader=user -->
<!-- maps: docs/DESIGN.md reader=user -->
<!-- maps: docs/ACCEPTANCE.md reader=user -->
<!-- maps: docs/DECISIONS.md reader=user -->
<!-- maps: docs/ARCHITECTURE.md reader=user -->
<!-- maps: docs/CAPABILITIES.md reader=user -->
<!-- maps: docs/INSTALL.md reader=user -->
<!-- maps: docs/PRODUCT-COMPARISON.md reader=user -->
<!-- maps: docs/DEVELOPMENT.md reader=developer -->
<!-- maps: tools/README.md reader=developer -->
<!-- maps: verification/README.md reader=evidence -->
