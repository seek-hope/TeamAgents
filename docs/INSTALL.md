# Installation, upgrade and uninstall

## 1. Build from source

Install the pinned toolchain in `rust-toolchain.toml`, then:

```bash
make check CARGO_FLAGS=--locked
make build
engine/target/debug/teamagents init
engine/target/debug/teamagents doctor
engine/target/debug/teamagents
```

`init` writes a minimal config and preserves an existing one. Set the credential environment variable named by
your model profile's `api_key_env`; never put credentials in the repository or prompts. `doctor` makes no model
requests. `doctor environment` discovers tools without executing them.

The binaries are `teamagents` (engine/CLI) and `teamagents-tui`. Use the same build for both.

## 2. Configuration and state

Defaults:

- Config: `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`
- Sessions: `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`
- Versioned installs: `~/.local/lib/teamagents`
- Launchers: `~/.local/bin/teamagents` and `~/.local/bin/teamagents-tui`

The default model profile is `leader_main`:

```toml
[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
context_window = 1000000
api_key_env = "DEEPSEEK_API_KEY"
generation_options = { reasoning_effort = "high" }
```

See [CONFIG.md](CONFIG.md) for provider and permission settings. Shell, acceptance commands and local MCP run on
the host. Retired sandbox keys are rejected rather than silently converted into host execution.

## 3. Upgrade

1. Review the target source, schema compatibility and [acceptance limits](ACCEPTANCE.md). Build both binaries
   before changing the installation. Preserve old binaries and back up any session you cannot afford to lose.
2. Finish work or explicitly terminate relevant instances. `Ctrl+Q` only detaches the TUI, and task cancellation
   does not stop a running command. A graceful daemon stop can let committed runners finish; it is not a
   substitute for stopping active work.
3. Stop each affected daemon:

   ```bash
   teamagents --state-root /path/to/session-state daemon --stop
   ```

4. Install both binaries from the same build, then run `teamagents version` and `teamagents doctor` before
   reconnecting to the intended session.

`init` does not merge new settings into an existing config; review and edit it explicitly.

The current database is **schema 9**. Migration preserves existing work/history, derives the Leader role and adds
the session exploration-tree projection. An unexpected pre-existing tree table is refused without modifying the
original store. Older writers cannot reopen the migrated database. A binary rollback alone is not a database
rollback; retain a pre-upgrade backup if rollback matters.

## 4. Governed updater

`teamagents update` provides a versioned transaction with health-check rollback and a journal. The exact command
shapes are documented by `teamagents update --help`.

Use a separate versioned installation root, `--bin-dir` and `--state-root` for isolated rehearsal. Agent update requests are disabled by
default; when enabled, they still require a concrete user approval and cannot be authorized by the automatic
reviewer. Publication is a separate operation.

A local `make release-rehearsal` builds and checks an archive and performs an isolated install/init smoke. It is
not a clean-host, real-service or public-release acceptance result.

## 5. Uninstall

Finish or stop work and daemons, then remove only the launchers:

```bash
rm -f ~/.local/bin/teamagents ~/.local/bin/teamagents-tui
```

Configuration remains under `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents`; sessions remain under
`${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`. Versioned installations may also retain binaries and an update
journal. Do not delete those directories or existing sessions as a generic cleanup step.
