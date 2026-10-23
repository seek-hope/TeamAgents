> TeamAgents **v0.2.0** is the first public release: Linux x86_64, musl-static, and no Rust toolchain
> is needed to run the published archive.

### What is in this release

- The single authoritative state: one SQLite database per session (WAL with `synchronous=FULL`); instances,
  tasks, grants, budgets, approvals, receipts and events commit in one transaction, and crash recovery is
  classified by persisted location instead of replaying side effects on a guess.
- One daemon owns the session; the TUI and `teamagents exec` are thin clients of its Unix-socket JSON
  protocol and reconnect from the last event watermark.
- Governed collaboration: `spawn` / `delegate` / `send` / `wait` are authorized by the control plane and
  re-checked at dispatch, with workspace policies (shared, isolated, own Git worktree) and retirement when an
  instance terminates.
- Mixed providers: instances speaking chat-completions, Responses or Anthropic can share one session, each
  with its own model, effort and native context window.
- User hooks: `[hooks] notify` forwards events to your own program and `pre_tool` can veto a tool call before
  it runs.
- Mermaid, SVG and DOT diagrams plus local raster images in the conversation and session history; native
  Kitty graphics, portable Unicode/block previews, and Ctrl+G source fallback.
- Local command completion with explicit submission, language syntax colors and viewport-driven media
  previews that remain available beyond sixteen images in long histories (D-450).
- Explicit engine-owned file/image snapshots and path completion, pending-input edit/cancel, local
  Markdown/JSON export and session-browser search (D-451). Display grammar/image/diagram parsing runs in
  resource-bounded terminable Rust helpers.
- Read-only work diagnostics with explicit recovery instructions in CLI/TUI (D-461); observation never
  prompts a model, settles a task or stops an operation.
- Two sandbox modes: `none` (the default, explicit host execution) or `approved_scope` with bubblewrap,
  Docker or SSH; a configured backend fails closed when unavailable (D-466).
- Repository `AGENTS.md` discovery from the trusted boundary to the cwd (D-467), bounded skill support
  resources (D-468), and disjoint multi-file edits plus parallel read-only batches (D-469).
- Session controls: the Leader model/effort menu (D-473), an explicit `after = "turn"` follow-up queue
  (D-470), web domain grants, and cancel-versus-stop diagnosis; Ctrl+Q quits, Ctrl+P opens the queue and
  Ctrl+M opens the model/effort menu.
- Terminal-independent `make pty` matrix across xterm/screen/tmux/Kitty profiles (D-472); acceptance checks
  follow the session's shell mode (D-471); the generated profile defaults to `high` reasoning effort (D-474).
- Per-model reasoning-effort menus (D-476): `[models.<key>] efforts` declares the values Ctrl+M offers,
  defaulting to minimal/low/medium/high/xhigh/max; a model that lacks the current level falls back to the
  nearest declared level at or below it, and the CLI `--effort` remains an explicit override.
- Installer, `teamagents init`, `doctor` and `exec` as documented in the
  [install guide](https://github.com/seek-hope/TeamAgents/blob/main/docs/INSTALL.md).

Local regression: `make check` green (core 147 / engine 415 / tui 85) plus `make pty`;
the tag workflow smoke-installs the exact archive before publishing it. Real-model acceptance is reported
separately in [docs/ACCEPTANCE.md](https://github.com/seek-hope/TeamAgents/blob/main/docs/ACCEPTANCE.md);
a release packages the current source and does not widen those evidence boundaries.

### Install and first run

The repository and its release archives are public, so no GitHub login is needed. The installer is the
recommended path:

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

It downloads the latest release, verifies SHA-256 and installs both programs; running it again upgrades them
and keeps the existing config and sessions. Add `export PATH="$HOME/.local/bin:$PATH"` to `~/.bashrc` or
`~/.zshrc` so later terminals find them.

You can also download `install.sh`, the archive and `SHA256SUMS` from the Assets section of this page and run
`sh install.sh --archive ./teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz` (replace VERSION). Use
`--version VERSION` or `--bin-dir DIR` to pin a version or a directory.

### Requirements

- Linux x86_64; the archives are statically linked against musl, so no Rust toolchain is needed.
- Shell isolation needs `bubblewrap`: on Debian/Ubuntu use `sudo apt install bubblewrap`.
- `init` writes a minimal config, never a credential and never over an existing file; the default model is
  DeepSeek Flash and other services are configured by editing the TOML.

Full install, upgrade and troubleshooting notes are in the
[install guide](https://github.com/seek-hope/TeamAgents/blob/main/docs/INSTALL.md).
