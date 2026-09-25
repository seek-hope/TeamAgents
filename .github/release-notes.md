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
- Installer, `teamagents init`, `doctor` and `exec` as documented in the [install guide](https://github.com/seek-hope/TeamAgents/blob/main/docs/INSTALL.md).

Local regression for this release: `make check` green (core 91 / engine 136 / tui 29) plus `make pty`;
real-model acceptance is reported separately in [docs/ACCEPTANCE.md](https://github.com/seek-hope/TeamAgents/blob/main/docs/ACCEPTANCE.md).

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
