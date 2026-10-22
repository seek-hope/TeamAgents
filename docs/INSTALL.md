# Installing, first run and upgrades

TeamAgents currently targets **Linux x86_64**. **No public release is published**: all release tags/assets
were removed on 2026-10-18 (D-457), and publication requires explicit user authorization. Build the current
source; do not follow historical latest-release or old-version download instructions.

## 1. Build and install from source

Run these commands from the repository root. Rust is pinned by rust-toolchain.toml; Cargo builds core as
a dependency of the two executable crates. The first build may download the toolchain and locked dependencies.

```bash
cargo build --locked --release --manifest-path engine/Cargo.toml --bin teamagents
cargo build --locked --release --manifest-path tui/Cargo.toml --bin teamagents-tui
mkdir -p ~/.local/bin
install -m755 engine/target/release/teamagents tui/target/release/teamagents-tui ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

Add `--offline` once dependencies are cached. Both binaries must come from the same source and be installed
in the same directory. Before replacing an existing installation, finish or explicitly stop its work and
follow section 3; closing the TUI alone does not stop the daemon. If the destination uses a versioned
installation with symlinks, preserve that layout rather than copying through its active links.

The [README](../README.md#building-from-source) gives the short path. Build and test conventions are in
[DEVELOPMENT.md](DEVELOPMENT.md); no root Cargo workspace manifest is required.

### Local archives and future releases

The installer also accepts a locally built Linux x86_64 musl archive with its matching SHA256SUMS beside it:

```bash
# Replace VERSION with the version in your locally built archive.
sh install.sh --archive /path/to/teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz
```

This requires the matching installer and archive, not a nonexistent public download. A musl archive does
not need Rust, Python or Node.js merely to run the product; tools and MCP services may need their own
runtimes. The installer verifies checksums, installs both binaries together, never invokes sudo and never
edits shell startup files. Use `--bin-dir "$HOME/bin"` for another destination. A missing checksum or
incomplete archive is a refusal, not permission to bypass verification.

The [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md) records an unpublished installation
rehearsal on its pinned source. That is not acceptance of every later source build. A future public
release needs a fresh exact-artifact rehearsal and separate publication authorization.

## 2. First configuration and start

```bash
teamagents init
```

The config lives at `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`. Repeating init preserves
an existing file, including symlinks. A new file is mode 0600 and contains no credential values. Init also
prepares the state root and session database; it starts no daemon or conversation.

The generated model profile is leader_main with deepseek-flash, a 1,000,000-token context and high reasoning
effort. For another service, set the protocol, model, endpoint, credential variable and native context
window together; see [configuration](USER-GUIDE.md#2-configuration). Additional bindings are illustrated in
[examples/config.toml](../examples/config.toml). Never put API key values in tracked files.

The default `sandbox = "none"` needs no sandbox. If you select the bubblewrap backend, install it:

| Distribution | Command |
|---|---|
| Debian / Ubuntu | `sudo apt install bubblewrap` |
| Fedora | `sudo dnf install bubblewrap` |
| Arch Linux | `sudo pacman -S bubblewrap` |

```bash
export DEEPSEEK_API_KEY='your key'
teamagents doctor
teamagents --cwd /path/to/project
```

Omit `--cwd` to use the current directory for a new daemon. A client joining an existing daemon keeps that
session's workspace; use an explicit named session or another state root for another project.
`teamagents --help` lists management commands. For scripts, use `teamagents exec "Explain this repository"`;
`--json` returns a machine-readable report.

Doctor sends no model request. Fix required failures before starting; warnings identify optional or
unconfigured capabilities. User-home development environments are not automatically available in the
sandbox: Rust toolchain/cache mounts exist, but Node/Python/Go installations may need explicit environment
preparation. See [permissions and isolation](USER-GUIDE.md#4-permissions-and-isolation), not full_auto as an
implicit fallback.

## 3. Upgrade and uninstall

For a manual source upgrade:

1. Review the target source, schema compatibility and [acceptance limits](ACCEPTANCE.md); build both new
   binaries before touching the installation. Preserve the old binaries and make a consistent backup of
   any session you cannot afford to lose. A binary rollback is not a database-schema rollback.
2. Finish work or explicitly terminate the relevant instances. Ctrl+Q only quits the TUI; task cancellation
   does not stop a running command. A graceful daemon stop allows committed runners to finish, so it is
   not a substitute for stopping active work.
3. Stop each affected daemon using its state root, then replace both binaries from the same build:

   ```bash
   teamagents --state-root /path/to/session-state daemon --stop
   ```

4. Run `teamagents version` and `teamagents doctor`, then reconnect to the intended session. Init preserves
   existing configuration; it does not merge new fields into it. Edit those fields deliberately.

D-453 provides a separately tested, governed updater with health-check rollback and a journal. Agent
update requests are disabled by default and require the user's concrete approval when enabled; the
reviewer cannot authorize them. This mechanism does not create a release target while publication is frozen.
See the update configuration in [CONFIG.md](CONFIG.md) and D-453 in [DECISIONS.md](DECISIONS.md).

To uninstall a simple two-binary source installation, first finish/stop its work and daemons, then remove
only its launchers:

```bash
rm -f ~/.local/bin/teamagents ~/.local/bin/teamagents-tui
```

Configuration and sessions remain under `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents` and
`${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`. A versioned installation can also retain binaries and
its update journal. Do not broadly delete those directories or existing sessions as part of uninstall.

## 4. Troubleshooting

| Symptom | What to do |
|---|---|
| Command not found | Add the chosen bin directory to PATH in your terminal and shell startup file |
| TUI binary missing or mismatched | Install both binaries from the same source into the same directory |
| Config/model missing | Run init and check the config path printed by doctor |
| Credential missing | Export the configured variable in the terminal that starts the daemon |
| Public download returns 404 | Expected while no release is published; build from source |
| Local archive checksum fails | Stop; obtain the matching archive/checksum pair and verify provenance |
| Bubblewrap probe fails | Check kernel/container restrictions on unprivileged namespaces; no host fallback occurs |
| Build tool exists on host but not in sandbox | Check workspace-local/system installation and the documented toolchain mounts |
| macOS / Windows / Linux ARM | No supported distribution is established by the Linux x86_64 checks |
| Wrong workspace after changing directory | You joined an existing session; select the intended named session or state root |
| State path is wrong | The state root is a directory, not the session.sqlite file; empty path arguments are refused |
