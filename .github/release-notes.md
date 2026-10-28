> These notes describe current source for a future release. They are not the body of the published v0.2.0
> release and do not authorize a new tag or publication.

### Current source highlights

- One authoritative SQLite database per session, with tasks, grants, budgets, approvals, receipts and events
  committed together. Recovery reuses verified results and records unknown outcomes without blind replay.
- One daemon per session. The TUI and `teamagents exec` use its Unix-socket JSON protocol and resume from the
  last event watermark.
- Governed teams through `spawn`, `delegate`, `send` and `wait`, with shared, private and Git-worktree
  workspaces and explicit task-scoped capability delegation.
- Chat Completions, DeepSeek, Responses and Anthropic profiles in one session, with per-model native context
  windows and configurable reasoning efforts.
- Host Shell, acceptance commands and local MCP services. Ordinary Shell needs no per-call approval; task
  capability checks and self-update approval remain. Retired sandbox settings are rejected during migration.
- File tools, persistent Shell state, DeepSeek web tools, Skills, stdio/HTTP MCP and QuickJS `codemode`
  composition. Hooks can report events or veto native tool calls.
- Trusted `AGENTS.md` discovery, bounded Skill support files, disjoint multi-file edits and parallel read-only
  batches.
- Attachments, pending-input editing, follow-up queues, session search/fork/export, an external editor and
  read-only work diagnostics with explicit recovery instructions.
- Markdown and syntax-colored code, Mermaid/SVG/DOT diagrams and local image previews. Kitty graphics and
  portable text/block fallbacks share a source view.
- Host environment discovery, boot-scoped config/source observations and input-delivery receipts. Delivery
  does not imply task completion, and configuration changes still require an explicit daemon restart.
- Ctrl+Q closes the TUI without stopping the daemon, Ctrl+P opens pending input and Ctrl+M opens the Leader's
  model/effort menu. `make pty` covers five terminal profiles and both forced graphics modes.

Local regression: `make check` green (core 147 / engine 415 / tui 91) plus `make pty`. The release workflow
smoke-installs the exact archive before publication. Real-model acceptance is reported separately in
[docs/ACCEPTANCE.md](https://github.com/seek-hope/TeamAgents/blob/main/docs/ACCEPTANCE.md); packaging current
source does not widen those evidence boundaries.

### Install and first run

The repository and release archives are public. The recommended installer is:

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
export DEEPSEEK_API_KEY='your key'
teamagents doctor
teamagents
```

The installer downloads the latest published release, verifies SHA-256 and installs both programs. Running it
again upgrades them while retaining existing config and sessions. Add the `PATH` export to `~/.bashrc` or
`~/.zshrc` for later terminals.

To install an archive from this release's Assets, download `install.sh`, the archive and `SHA256SUMS`, then run
`sh install.sh --archive ./teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz` after replacing `VERSION`.
Use `--version VERSION` or `--bin-dir DIR` to select a published version or destination.

### Requirements

- Linux x86_64. The archives are statically linked against musl and need no Rust toolchain at runtime.
- Current source uses host Shell and local MCP; no sandbox package is required. Host commands have the user's
  filesystem and network access.
- `init` creates a minimal config without credentials and never overwrites an existing file. The default
  profile is DeepSeek Flash; edit the TOML to use another service.

See the [install guide](https://github.com/seek-hope/TeamAgents/blob/main/docs/INSTALL.md) for source builds,
upgrades and troubleshooting.
