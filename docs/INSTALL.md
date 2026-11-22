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

The starter profile is `leader_main`. Set `default_model` above the first table in the user config to choose another profile for new sessions.

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

Source builds use **schema 13**. The 12→13 migration creates empty workspace snapshot and restore-attempt tables.
Existing conversation checkpoints continue to work with their original data. The 11→12 step fences off older writers that would ignore session MCP
restrictions. It creates an empty rewind-point table and allows multiple reviewed envelopes to reference the
same immutable attachment, preserving existing attachment rows. It does not infer past checkpoints. Untouched
bindings retain their configured behavior. The 10→11 step marks existing Workers persistent without rewriting historical
envelopes or inferring which task an old request addressed. New Worker turns record their assignment before
provider dispatch. A legacy finish with no recorded assignment cannot settle a task by queue position;
inspect the outstanding task before resuming. Earlier writers cannot enforce after-work delivery.
The 9→10 migration preserves instance profiles, work, history and prepared requests. A store with live Workers needs an explicit shared model decision; other stores start in follow mode.
Earlier migrations derive the Leader role and add the session exploration-tree projection. An unexpected pre-existing tree table is refused without modifying the
original store. Older writers cannot reopen the migrated database. A binary rollback alone is not a database
rollback; retain a pre-upgrade backup if rollback matters.

## 4. Governed updater

`teamagents update` provides a versioned transaction with health-check rollback and a journal. The exact command
shapes are documented by `teamagents update --help`.

Normal clients use socket protocol 2. The current updater can inspect and stop protocol-1 and protocol-2
daemons through the unchanged checkpoint/shutdown envelopes; ordinary operations still require protocol 2.
A live incompatible listener is an error, not a successful stop. The current updater also reads the JSON
`version` field emitted by the real CLI.

An older updater may report `staged binary reports "}", expected ...` or reject the newer daemon protocol.
Use the verified new archive's extracted CLI to run the transition with the same installation paths:

```bash
/path/to/extracted/teamagents update --to 0.2.0 --archive /path/to/release.tar.gz \
  --install-root /path/to/versioned-install --bin-dir /path/to/bin --state-root /path/to/session-state
```

Keep the archive's original release filename and adjacent `SHA256SUMS`. `--rollback` switches to the previous
binary set; if a newly started daemon fails health checks, the transaction attempts to restore the preceding set.
If that restoration also fails, the error and journal retain both failures. `--no-restart` switches the binaries
while preserving the running daemon. On Linux the daemon starts each new Shell runner from its own mapped
executable (`/proc/self/exe`), so replacing the installation path does not strand a running session. If an older
daemon reports `spawn runner: No such file or directory`, stop it at a quiet boundary and reopen the same session
with the fixed CLI. The configuration and session history remain in their existing paths.
The database-schema limitation above still
applies, and old installed updater code does not gain new compatibility from downloading an archive alone.

Use a separate versioned installation root, `--bin-dir` and `--state-root` for isolated rehearsal. Agent update requests are disabled by
default; when enabled, they still require a concrete user approval and cannot be authorized by the automatic
reviewer. Publication is a separate operation.

A local `make release-rehearsal` builds and checks an archive and performs an isolated install/init smoke.
For the package and terminal checks, see [ACCEPTANCE.md](ACCEPTANCE.md#workflow-checks). Clean-host
two-version installation/update/rollback, schema transitions, physical hosts, native ARM and broader platform
coverage require separate acceptance. These checks neither establish real-model quality nor publish a package.

## 5. Uninstall

Finish or stop work and daemons, then remove only the launchers:

```bash
rm -f ~/.local/bin/teamagents ~/.local/bin/teamagents-tui
```

Configuration remains under `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents`; sessions remain under
`${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`. Versioned installations may also retain binaries and an update
journal. Do not delete those directories or existing sessions as a generic cleanup step.
