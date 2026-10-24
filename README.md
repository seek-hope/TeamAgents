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
| [Daily coding readiness and comparison](docs/PRODUCT-COMPARISON.md) | current Pi comparison, evidence limits and prioritized candidate improvements; dated Codex reference |
| [Acceptance](docs/ACCEPTANCE.md) | Per-item evidence for the A01–A36 acceptance matrix, plus known gaps |
| [Capabilities](docs/CAPABILITIES.md) | One row per capability: the check behind it, the boundary that check measured, and what is still open |
| [Design baseline](docs/DESIGN.md) | Confirmed requirements, architecture and protocol constraints, A01–A36, definition of done |
| [Development](docs/DEVELOPMENT.md) | Toolchain, gates, layout conventions, re-run commands |
| [Decisions](docs/DECISIONS.md) | Confirmed direction, boundaries and deviations |
| [Install guide](docs/INSTALL.md) | Download, install and upgrade |
| [Formal verification](verification/README.md) | TLA+ specs and Kani proofs: properties ↔ code ↔ acceptance items, unproven list |

<!-- The reading map (D-443): every tracked document declares its reader and exactly one home, the map below
     points at it, and review/doc_inventory.py checks the three directions. The entry points need no line. -->
<!-- maps: docs/ACCEPTANCE.md reader=evidence -->
<!-- maps: docs/CAPABILITIES.md reader=evidence -->
<!-- maps: docs/DECISIONS.md reader=evidence -->
<!-- maps: docs/DESIGN.md reader=developer -->
<!-- maps: docs/INSTALL.md reader=user -->
<!-- maps: docs/PRODUCT-COMPARISON.md reader=user -->
<!-- maps: docs/USER-GUIDE.md reader=user -->

> **First time here:** [install guide](docs/INSTALL.md). The first public release is **v0.2.0** for Linux x86_64.

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

- The TUI talks to the Leader; whether to split work and how many instances to use is the Leader's call
  (a team of one is legal). Direct member input and intervention are available through the developer CLI.
- Every instance has its own model, tool bindings, workspace policy and budget. Work that exceeds its scope
  becomes an **approval request** while the rest of the team keeps working.
- Runtime facts (who is doing what, where a task is stuck, why a turn ended) all land in the session
  database: queryable, recoverable, auditable.

## Highlights

- **Governed collaboration**: `spawn` / `delegate` / `send` / `wait` are authorized by the control plane and
  re-checked at dispatch; the session grants the Leader its team authority by default (`manage` / `delegate` /
  `message` over the session), and a grant can be revoked at any time — revocation cascades, and limited
  delegation, timeouts and unknown outcomes all have recovery classifications. The tool surface follows the
  grants, so an instance is offered only what it can dispatch. A spawned worker has no ambient shell;
  D-455 adds explicit task-scoped delegation from a user-issued delegable grant, or a pending capability request.
- **Workspace policies**: `spawn` can give an instance the shared project directory, a private isolated
  directory or its own Git worktree and branch. Termination retires the workspace from its record; a
  directory with uncommitted or unmerged work is never deleted automatically, only reported. A worktree's
  branch is yours to merge (name and base commit in `<state root>/instances/<id>/worktree.json`); once it is
  merged, the running session retires the checkout by itself.
- **Mixed models**: real instances speaking the three wire protocols (responses / anthropic /
  chat-completions) can share one session, each with its own model, effort and native context window; `spawn`
  takes the catalog entry for a worker (key or model name), so one team can mix entries. An instance whose
  entry is unknown is refused as that tool call's error, and one that can never boot is parked with the reason
  instead of stopping the session.
- **Single source of truth**: one SQLite database per session (WAL with `synchronous=FULL`). Crash recovery
  is classified by persisted location — known results are reused, in-flight losses are recorded honestly
  (`OUTCOME_UNKNOWN`), and nothing is **replayed on a guess**.
- **Permission gate**: `[permissions] sandbox` selects two modes — `none` (the default: host shell, no shell
  approval) or a backend (`bubblewrap`/`docker`/`sbx`/`ssh`: approvals for out-of-scope work); an approval binds one concrete operation, its argument hash and its
  permission revision, so it is used up by that single dispatch.
- **Completion gate**: `finish` only accepts honest outcomes; the `[[checks]]` in **your own** config must
  actually pass before a goal can settle successfully. Project configuration is loaded when trusted, but
  cannot supply checks: acceptance policy remains user-owned (see *Configuration and team*).
- **Long-context compaction**: triggered by real window usage; the summary keeps the original request, user
  revisions, acceptance criteria and open questions, while the full text stays retrievable through
  `read_history`. Compaction calls count against the goal budget.
- **Tools**: file read/write/search with exact-match edits, SHA-256 version checks and atomic writes (a
  process-wide write lock serializes mutations), a persistent in-session shell (`cd` and
  `export` survive across commands), web search and fetch via DeepSeek Search/Fetch, MCP (stdio and streamable HTTP, composed from
  model-written code through the one `codemode` tool, D-374) and Skills
  (`~/.agents/skills`).
- **User hooks (`[hooks]`)**: `notify` forwards events (`tool_call` / `team_action` / `run_*`) to your own
  program; `pre_tool` can veto any native tool call before it runs (exit 2 denies, stderr is the reason),
  and a broken hook only logs instead of stalling the team.
- **Isolation**: `sandbox = "none"` (the default) runs on the host by design; a configured backend runs
  instance shells inside it, and when it is missing the command fails loudly instead of silently degrading.

D-451 adds explicit text/image attachments (`/attach PATH` or `exec --attach PATH`), workspace path
completion, revision-checked pending input (`/queue` or `inputs`), browser search and complete local
Markdown/JSON session export. Rendering runs in bounded Rust display processes with safe source fallback.
See the [user guide](docs/USER-GUIDE.md) for limits and controls and the [maturity report](review/maturity-2026-10-11/REPORT.md)
for its pinned deterministic local evidence. Current source also includes an external editor, task-scoped
capability delegation, optional operation auto-review and provider cache telemetry. v0.2.0 is the first public release.

## Install

Requirements: Linux (x86_64); a sandbox backend is optional (the default is `none`). Model credentials are read from environment variables and
never written into the config.

### Install the published release

The first public release is **v0.2.0** for Linux x86_64 (musl static; no Rust toolchain needed to run):

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

The installer resolves the latest release, verifies the published `SHA256SUMS`, and installs
`teamagents` and `teamagents-tui` together. Running it again upgrades them and keeps the existing
config and sessions. See the [install guide](docs/INSTALL.md) for `--version`, `--bin-dir` and local
archives.

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

The default `sandbox = "none"` needs no sandbox. If you want the bubblewrap backend, install it
(Debian/Ubuntu: `sudo apt install bubblewrap`; other distributions are in the install guide), then run in one terminal:

```bash
teamagents init                       # write a minimal config and prepare the state root; never overwrites
export DEEPSEEK_API_KEY='your key'    # the default config uses DeepSeek; other services follow api_key_env
teamagents doctor                     # config / credentials / state root / skills / isolation probes
teamagents --cwd /path/to/project     # your project directory; omit --cwd to use the current one
```

`init` follows the XDG layout; the default model is DeepSeek Flash (1M context) and the generated TOML can be
edited for another service. Existing configuration is preserved; review it with `doctor` rather than assuming
that init merges new fields into it.

Then just state your goal:

```
Make the tests under /tmp/proj pass and explain why each file changed.
```

The Leader decides whether to split the work and who does what. It only interrupts you for approvals
(out-of-scope commands, network access). The TUI keeps one conversation with the Leader. Use `Ctrl+A`
for pending approvals; developer management and diagnostics live in `teamagents --help` (D-445).

For scripts and CI, use the headless entry point (`exec` shares the daemon with the TUI; `-` reads the
prompt from stdin, so a long instruction can be piped in):

```bash
teamagents exec "What is 1+1? Answer directly."                  # one input, print the reply
teamagents exec --json --timeout 180 "Make /tmp/proj tests pass"  # machine-readable summary
teamagents exec --stream-json "Get the tests green" | jq -c '.event.kind'  # the events while it runs
teamagents exec --check "cargo test --offline" "Get the tests green"  # plus your own acceptance check
teamagents exec --accept tests="pytest -q" "Fix the failing tests"    # the goal cannot settle until it passes
git diff | teamagents exec -                                      # the prompt comes from stdin
```

`--stream-json` writes one JSON object per line to stdout: `{"type":"event","event":{…}}` for each event of the
session as the run observes it (in log order, each once, flushed as it is written) and then the report as
`{"type":"report","report":{…}}` — the same object `--json` prints. The exit codes are the same, and a consumer
that closes the pipe (`| head`) ends the stream, not the run.

Exit codes: `0` settled, `1` failed or unfinished, `3` an approval is pending (a headless run has nobody to
answer it, so it reports instead of waiting), `124` the `--timeout` deadline passed, `2` usage or
infrastructure (no daemon, no model profile). Each `--check COMMAND` runs after the turn ends, in order, through the session's shell mode (`sandbox =
"none"`, the default, is the host) inside your workspace (`--cwd` or the current directory); the first failure
stops the list
and makes the run fail. The verdicts are printed, written to `<state root>/verification.json` and included in
the `--json` report as `verification`. Each `--accept ID=COMMAND`, by contrast, is attached to the *goal*
before the input lands (`teamagents goals open --check ID=COMMAND` does the same for a goal you open yourself),
so the runtime runs it at the completion boundary and sends a failing turn back to work before the goal can
settle — the same machinery `[[checks]]` feeds (D-385).

**Your session, your ceilings.** `[limits]` in the config bounds every goal — `max_total_tokens` refuses a
request that would pass the usage ceiling and `deadline_minutes` refuses one past the deadline; without them a
session runs until you stop it, which `doctor` says out loud. See
[docs/USER-GUIDE.md](docs/USER-GUIDE.md) §2.2.

**Capabilities are yours to hand out.** Workers do not inherit ambient authority. You can grant one worker
its shared-workspace shell, or explicitly allow the Leader to delegate it for tasks that request it:

```bash
teamagents authority                                    # grants and pending capability requests
teamagents authority grant --subject i-worker-1 --action shell --scope workspace
teamagents authority grant --subject i-leader --action shell --scope workspace --delegable
teamagents authority revoke --grant g-1a2b3c4d          # final; takes derived grants with it
```

The delegable option is a user choice, not a default. A child grant cannot widen its parent or be delegated
again, and expires with its task or instance. Without a covering parent, declared capability needs wait for
the user's decision instead of starting a worker that cannot execute its task.

Grants are checked again at every dispatch, so a revocation stops work that was already queued, and the tool
leaves the model's surface at its next request. The surface refuses what could never do anything (an unknown
action, or a pair such as `shell@instance:i-worker-1` that no check asks about) and tells you what to use
instead. Details, the full action vocabulary and the exit codes are in
[docs/USER-GUIDE.md](docs/USER-GUIDE.md) §3.1.

## Common arguments and keys

| Usage | Meaning |
|---|---|
| `--cwd DIR` | work in DIR (default: the current directory); it applies to the session this command starts — a running session keeps its own workspace, and the client prints that one instead of working somewhere else. A path that is not an existing directory is refused before anything starts (the session confines every file and shell command to it) |
| `--state-root PATH` | use a specific state root (default `$XDG_STATE_HOME/teamagents/v2`); it is the *base* directory: the default session's `session.sqlite`, `daemon.sock` and `daemon.log` live directly in it, and `sessions new` puts named sessions under `sessions/<id>/` (D-364). A path that is a file is refused with the flag named (`doctor`, `init`, `daemon`, `exec` and the session verbs), and one that cannot be created, or that holds a *directory* where `session.sqlite`, `daemon.sock` or `daemon.log` belongs, is refused with the flag and the fix (`init`, `daemon`, `exec`) |
| `--session ID` | attach to a named session recorded in that base (D-364); `teamagents sessions` lists the ids. It resolves to the session's own state root, so it works with the TUI, `exec`, `daemon`, `init` and `doctor`; an id that is not recorded (or is archived) is refused with the base named |
| `--model KEY` | pick a model catalog entry (default `leader_main`) |
| `--every MINUTES` / `--prompt TEXT` | `automations add`: the period and the prompt each run submits (D-367) |
| `--query TEXT` / `--limit N` | `sessions search`: the substring to look for and the most hits to return (default 20, D-368) |
| `--full-auto` | user-only full-auto mode (out-of-scope work is approved instead of requested); it applies to the session this command starts — a session that is already running keeps the mode it booted with, and the client prints that mode instead of pretending |
| `init` / `doctor` / `daemon` / `daemon --stop` / `exec` / `authority` / `approvals` / `instances` / `tasks` / `runners` / `artifacts` / `sessions` / `automations` / `version` / `--help` | config and state root / self-check (config, credentials, state root, skills, goal limits, checks, hooks, sandbox mode) / run the daemon alone (its output goes to `<state root>/daemon.log`) / stop the session that owns this state root, addressed by its socket (no pid; `--cwd`/`--model`/`--full-auto` are refused) / headless input / list, grant and revoke capabilities / list and decide approvals / pause, resume, interrupt, terminate, merge a worktree member's branch and list instances / list and cancel tasks / list the job runners a state root still carries and ask the settled ones to retire / list a state root's artifacts (bytes, owner, presence) and collect the ones nothing references (`runners stop --service --yes` instead signals the process group a settled command left behind) / list, create, fork, rename, restore, search, archive and delete named sessions under the base (D-364/D-365/D-368) / list, create, pause, resume and delete schedules that start a goal on their own (D-367) / version / usage |

Conversations render Markdown, fenced code, lists and tables, with distinct user messages and compact
tool results. `Ctrl+O` expands or folds tool previews; full stored content remains in the session page (D-447).
Mermaid, SVG and DOT diagrams and local PNG/JPEG/GIF/WebP/BMP/TIFF/ICO images also render in the conversation
and session history. `Ctrl+G` switches previews/source. Kitty displays images; other terminals use text diagrams
or colored blocks. `TEAMAGENTS_GRAPHICS=auto|kitty|blocks|off` selects the display mode (D-448).
D-450 adds language syntax colors and decodes only visible media, so later images in long histories can load.
A slash prefix opens local completion: Up/Down selects, `Tab` or Enter fills, and `Esc` keeps the draft.
Press Enter to submit the filled command.

TUI keys: `Enter` sends to the Leader; `Ctrl+J` or `Shift+Enter` inserts a newline. `↑`/`↓` recalls
input and walks multiline drafts; `PageUp`/`PageDown` and the wheel scroll history. `Ctrl+W` deletes a word;
`Ctrl+←`/`Ctrl+→` moves by word. `Ctrl+A` focuses pending approvals: `a` approves once, `d` denies,
`Esc` returns to the draft. `Ctrl+C` interrupts the Leader's model request (running tools and delegated
work continue); `Ctrl+Q` or `/quit` quits the TUI while the session keeps running. `Ctrl+M` or `/model`
opens the Leader's model and reasoning-effort menu (arrows choose, Enter applies at the next turn, Esc
cancels); `Ctrl+P` or `/queue` lists pending inputs. `Ctrl+S` or `/sessions` opens session history and
switching: select with arrows, Enter switches, brackets page history, and `d` then `y` deletes the selected
session. Deleting the current session succeeds before the TUI quits. Esc cancels/returns.
Client commands: `/help`, `/sessions`, `/interrupt`, `/quit`, `/model`, `/attach`, `/detach`, `/queue` and `/edit`.
`Ctrl+E` opens the draft in `VISUAL` or `EDITOR`; successful edits return to the composer without sending.
The `/edit` suffix after draft text opens the same editor; `/edit` alone explains it and is not in the
completion menu. Drafts survive resizing and session switches; approvals never take typing focus.

Developer inspection: `teamagents status --json` reads the running session's checkpoint;
`teamagents history --id INSTANCE --limit 400 --json` reads a bounded tail of stored messages, including
tool records. Member/task/goal controls, authority, session management, MCP and diagnostics are available in the CLI;
see the [developer command map](docs/USER-GUIDE.md#1-quick-start).

## Configuration and team

- A session reads your `$XDG_CONFIG_HOME/teamagents/config.toml` and discovers the nearest ancestor
  `.teamagents/config.toml`. Project settings contribute **nothing** until you trust that canonical project
  root through `[permissions] trusted_projects`, or explicitly trust all projects with `trust_project = true`.
  Models, tools, skills paths and instruction files then merge in (your definitions win); permissions, hooks,
  checks, retention, limits and update policy stay yours. `doctor` reports the trust/merge result.
  Instruction files may be listed explicitly; a trusted repository's `AGENTS.md` files are also discovered from
  its boundary down to the working directory (D-467). The sandbox may be none, bubblewrap, Docker, Docker Sandboxes (`sbx`) or SSH.
- A model profile selects its wire format with `protocol = "responses" | "anthropic" | "openai" |
  "deepseek"`; `base_url` plus `model` decide the actual service, and credentials are only referenced by
  environment-variable name.
- `[[checks]]` in the user config (never in a project file) are the machine contracts a goal must satisfy:
  the runtime runs them through the session's shell mode at the completion boundary (D-471), so a failing check
  returns the work
  for repair instead of letting a summary claim success.
- The Leader forms the team at runtime (`spawn` / `delegate` / `send` / `wait`); there is no static team
  definition file.
- Tool bindings as authorization, the approval semantics, Skills/MCP, retention and failure handling are in
  the [user guide](docs/USER-GUIDE.md).

## Status and limits

- Linux x86_64 is the supported distribution target; v0.2.0 is the first public release. There is no supported
  Windows/macOS build, distributed instance protocol or built-in browser automation.
- A selected sandbox backend fails closed when unavailable. SSH runs a remote
  shell, not a distributed agent, and does not guarantee remote cancellation or enforce network isolation.
- Controlled daily coding use is possible; equivalence to Pi as a sole general-purpose driver remains
  unestablished. Read-only work diagnostics and explicit recovery are available in CLI/TUI (D-461), without
  automatic model reminders. Remaining priorities are development/skill environments and paired coding
  evidence, followed by instruction discovery, tool composition, session controls and complete accounting.
- The four protocol families of DESIGN §7 (Chat Completions — the `openai` and `chat/completions` names —
  DeepSeek extensions, Anthropic and responses) each have a local fake-server regression test
  (`engine/tests/providers_fake.rs`), and `python3 review/dogfood/protocols.py` accepts each one against a real
  service. A family whose credential is not in the environment is reported as **skipped** (`--strict` turns that
  into a failure), so how many are accepted live depends on the credentials of the machine running it (D-151).
- Acceptance boundaries and evidence live in [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md), which lists every open
  item and known ceiling.

## Development

Run `make check` before anything else; the full workflow is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) and
the repository conventions in [AGENTS.md](AGENTS.md). Defects, reviews and evaluation evidence live in
`review/`, formal-verification evidence in `verification/`. A `v*` tag builds the release archive, but publication is gated by the
`TEAMAGENTS_RELEASE_APPROVED` repository variable; v0.2.0 is published, and future releases remain gated by that variable.

Chinese documentation: [README.zh-CN.md](README.zh-CN.md)
