# Installing, first run and upgrades

TeamAgents currently supports **Linux x86_64**. Release archives are statically linked against musl and need
neither Rust, Python nor Node.js. Model services require your own credentials.

## 1. Download and install

Run the [install command from the README](../README.md#latest-release-recommended): it picks the latest
release, verifies SHA-256 and installs `teamagents` and `teamagents-tui` into `~/.local/bin`. Both programs
must come from the same version and live in the same directory.

The repository is public, so no GitHub account or clone is needed. The installer prefers a logged-in `gh` and
falls back to `curl`; it supports Linux x86_64 only and fails before downloading on anything else.

**Custom installs:** replace `sh "$installer"` from the README with any of these:

```bash
sh "$installer" --version 0.1.2             # pin a version (a leading "v" is accepted)
sh "$installer" --bin-dir "$HOME/bin"      # custom directory (TEAMAGENTS_BIN_DIR also works)
sh "$installer" --archive /path/to/teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz
```

A local install needs no network but requires the matching `SHA256SUMS` next to the archive. The installer
verifies only the archive for the current platform, so a manifest listing other platforms is fine. If
verification fails or the archive is incomplete, already installed programs stay untouched; if replacing the
second binary fails, the previous one is restored. The installer never calls sudo and never edits shell
startup files.

**Browser download:** open the [latest release](https://github.com/seek-hope/TeamAgents/releases/latest) and
download `install.sh`, the `.tar.gz` and `SHA256SUMS` into the same empty directory, then run:

```bash
# Replace VERSION with the version you downloaded.
sh install.sh --archive ./teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz
export PATH="$HOME/.local/bin:$PATH"
```

The v0.1.1 archives ship no installer; download `install.sh` from the source address used in the README and
install locally.

**Building from source:** see the [README](../README.md#building-from-source). Only `engine` and `tui` need
to be built; Cargo builds `core` automatically.

## 2. First configuration and start

The built-in template writes a minimal config, so there is no example file to hunt for:

```bash
teamagents init
```

The config lives at `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`. Running it again keeps any
existing file, symlinks included. The new file is mode `0600`, holds no credentials and creates no session.
The default profile is `leader_main` with the model `deepseek-flash` (context 1,000,000, reasoning effort
max). For another service, edit `provider`, `protocol`, `model`, `base_url` and `api_key_env` (see the
[user guide](USER-GUIDE.md#2-configuration)) and set that model's native `context_window` at the same time.
Only the credential's environment-variable name goes into the file; web search and MCP are configured from
the bundled `config.example.toml` when needed.

v0.1.1 has no `init`; its installer copies a template when the config is missing, so setting the credential
is enough.

Shell isolation needs bubblewrap — pick the line for your distribution:

| Distribution | Command |
|---|---|
| Debian / Ubuntu | `sudo apt install bubblewrap` |
| Fedora | `sudo dnf install bubblewrap` |
| Arch Linux | `sudo pacman -S bubblewrap` |

Set the credential and start in the same terminal:

```bash
export DEEPSEEK_API_KEY='your key'
teamagents doctor
teamagents --cwd /path/to/project
```

Replace `/path/to/project` with your project; omitting `--cwd` uses the current directory. Then just type a
goal. `teamagents --help` lists every argument. For a non-interactive terminal or a script use
`teamagents exec "…"` (`--json` prints a machine-readable summary).

`doctor` verifies the local machine and sends no model request. `FAIL` needs fixing; `WARN` marks an optional
capability. The diagnostics of v0.1.1 were coarse enough to report a missing Codex CLI as a failure; from
v0.1.2 on they are more precise, and the current version does not probe Codex at all.

## 3. Upgrade and uninstall

Quit a running TeamAgents, then run the README install command again: both programs update together. It never
overwrites the config or deletes sessions. Afterwards run `teamagents version` and `teamagents doctor`. New
config fields are edited by hand — `init` neither migrates nor merges an existing config. Installing an
earlier version rolls the binaries back, but cross-version session compatibility depends on that release's
notes.

Uninstall the programs:

```bash
rm -f ~/.local/bin/teamagents ~/.local/bin/teamagents-tui
```

The config and sessions stay behind in `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents` and
`${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`; delete those directories yourself only when you are sure
you no longer need them.

## 4. Troubleshooting

| Symptom | What to do |
|---|---|
| `teamagents: command not found` | Run `export PATH="$HOME/.local/bin:$PATH"` and add it to your shell's startup file (`~/.bashrc`, `~/.zshrc`) |
| `teamagents-tui` is missing | Install both programs into the same directory; do not copy `teamagents` alone |
| Config missing / no model configured | Create it as in section 2; the self-check prints the path it used |
| Credential unset or empty | Export the referenced variable in the same terminal that starts TeamAgents |
| Download 404 / no release | Check the version, the network and the release list; a private fork also needs GitHub account permissions |
| SHA-256 verification failed | Stop, clear this download and fetch the archive plus its checksum file again |
| bwrap is installed but the probe fails | The container, kernel or distribution restricts unprivileged user namespaces; member shells cannot run there |
| macOS / Windows / Linux ARM | No archive exists yet; do not try to run the x86_64 Linux build natively |
