# Installing, first run and upgrades

TeamAgents supports **Linux x86_64**. The published release is **v0.2.0**; current source may contain newer
changes. Release archives are statically linked against musl and do not need Rust at runtime.

## 1. Install TeamAgents

### Published release

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

The installer selects the latest release, verifies its checksum and installs `teamagents` and
`teamagents-tui` together. Continue with §2, or build from source below.

### Build and install from source

Run from the repository root. `rust-toolchain.toml` selects the Rust version, and Cargo builds `core` as a
dependency of the two executable crates. The first build may download the toolchain and locked dependencies.

```bash
cargo build --locked --release --manifest-path engine/Cargo.toml --bin teamagents
cargo build --locked --release --manifest-path tui/Cargo.toml --bin teamagents-tui
mkdir -p ~/.local/bin
install -m755 engine/target/release/teamagents tui/target/release/teamagents-tui ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

Add `--offline` after dependencies are cached. Install both binaries from the same source into the same
directory. Before replacing an existing installation, finish or explicitly stop its work as described in §3;
closing the TUI alone leaves the daemon running. If the destination uses versioned symlinks, preserve that
layout instead of copying through active links.

See the [README shortcut](../README.md#building-from-source) and
[development guide](DEVELOPMENT.md) for build and test conventions. There is no root Cargo workspace.

### Local archives

The installer accepts a Linux x86_64 musl archive with its matching `SHA256SUMS` beside it:

```bash
# Replace VERSION with the archive's version.
sh install.sh --archive /path/to/teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz
```

Use the installer that matches the archive. The product itself does not need Python or Node.js, although
individual tools and MCP services may need their own runtimes. The installer verifies checksums, never invokes
sudo and does not edit shell startup files. Use `--bin-dir "$HOME/bin"` to choose another destination. Missing
or invalid checksums stop installation; do not bypass that check.

The [D-451 installation rehearsal](../review/maturity-2026-10-11/REPORT.md) is pinned to its recorded source.
The v0.2.0 workflow also smoke-tested its exact archive before publication. Neither result is a promise about
every later source revision.

## 2. First configuration and start

```bash
teamagents init
```

The config is `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`. Repeating `init` preserves an
existing file, including symlinks. A new file has mode `0600` and contains no credential values. `init`
prepares the state root and session database without starting a daemon or conversation.

The generated `leader_main` profile uses `deepseek-flash`, a 1,000,000-token native context and high reasoning
effort. To use another service, update its protocol, model, endpoint, credential variable and native context
window together. See [configuration](USER-GUIDE.md#2-configuration) and
[examples/config.toml](../examples/config.toml). Never put API key values in tracked files.

Current source runs Shell and local MCP on the host; no sandbox package is required.

```bash
export DEEPSEEK_API_KEY='your key'
teamagents doctor
teamagents --cwd /path/to/project
```

Omit `--cwd` to use the current directory for a new daemon. A client joining an existing daemon keeps that
session's workspace; select another named session or state root for another project. For scripts, use
`teamagents exec "Explain this repository"`; `--json` prints a machine-readable report. `teamagents --help`
lists management commands.

`doctor` sends no model requests. Fix required failures before starting; warnings describe optional or
unconfigured capabilities. `teamagents doctor environment` discovers host development tools without running
them. Bash inherits a controlled host environment without sourcing login profiles. Remove retired sandbox
settings before starting a new daemon; see [permissions and execution](USER-GUIDE.md#4-permissions-and-execution).

## 3. Upgrade and uninstall

For a manual source upgrade:

1. Review the target source, schema compatibility and [acceptance limits](ACCEPTANCE.md). Build both binaries
   before changing the installation. Preserve the old binaries and back up any session you cannot afford to
   lose. A binary rollback does not roll back a database-schema migration.
2. Finish work or explicitly terminate the relevant instances. `Ctrl+Q` only closes the TUI, and task
   cancellation does not stop a running command. A graceful daemon stop lets committed runners finish, so it
   is not a substitute for stopping active work.
3. Stop each affected daemon by state root, then install both binaries from the same build:

   ```bash
   teamagents --state-root /path/to/session-state daemon --stop
   ```

4. Run `teamagents version` and `teamagents doctor`, then reconnect to the intended session. `init` does not
   merge new settings into an existing config; review and edit them explicitly.

Older `sessions/` layouts and pre-v2 configs are not migrated automatically. `doctor` reports the old
directory without adopting it. Inventory old data, choose a new state root and remove anything only through an
explicit separate operation. Retired sandbox keys must be removed deliberately; they are not silently
converted into host execution.

The governed updater in D-453 provides a separately tested transaction with health-check rollback and a
journal. Agent update requests are disabled by default and, when enabled, still require concrete user
approval. The reviewer cannot authorize them. Publication remains a separate operation; see
[update configuration](CONFIG.md) and [D-453](DECISIONS.md).

To uninstall a simple two-binary installation, finish or stop its work and daemons, then remove only the
launchers:

```bash
rm -f ~/.local/bin/teamagents ~/.local/bin/teamagents-tui
```

Configuration remains under `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents`; sessions remain under
`${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`. Versioned installations may also retain binaries and an
update journal. Do not delete those directories or existing sessions as a generic cleanup step.

## 4. Troubleshooting

| Symptom | What to do |
|---|---|
| Command not found | Add the chosen bin directory to `PATH` in the terminal and shell startup file |
| TUI missing or mismatched | Install both binaries from the same source into the same directory |
| Config or model missing | Run `teamagents init` and check the config path printed by `doctor` |
| Credential missing | Export the variable named by `api_key_env` in the terminal that starts the daemon |
| Public download returns 404 | Check the version, asset name and network; v0.2.0 is published |
| Local archive checksum fails | Stop and obtain a matching, trusted archive/checksum pair |
| Build tool missing | Run `teamagents doctor environment`, then inspect the host `PATH` and member exports |
| Retired sandbox setting is rejected | Remove the named `sandbox`/`sandbox_*`, `mcp_network` or `mcp_execution = "workspace"` setting explicitly |
| macOS, Windows or Linux ARM | No supported build is established by the Linux x86_64 checks |
| Wrong workspace after changing directory | You joined an existing session; select the intended named session or state root |
| State path is wrong | Pass the state-root directory, not the `session.sqlite` file; empty paths are refused |
