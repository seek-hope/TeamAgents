# TeamAgents

[中文](README.zh-CN.md) · [Design decisions](docs/DECISIONS.md) · [Architecture diagram](docs/architecture.canvas) · [Formal specification](docs/FORMAL_SPEC.md)

TeamAgents runs coding tasks in a local workspace. Each session starts fresh with isolated per-node conversation history—never bleeding context across unrelated sessions. A task starts with `default` unless you select another member. Each stage can finish the work or hand off assignments to child stages. When a round finishes with **1 terminal leaf**, it settles directly at that leaf; when **2+ leaves** finish, they converge into an in-tree `summarizer` node. Subsequent follow-up prompts send directly to that sole leaf or summarizer and promote it to the root of the next subtree round.

## Install and start

The shell sandbox supports native x86-64 and little-endian AArch64 Linux. Install `bubblewrap` and `slirp4netns` with your distribution's package manager (on Ubuntu, `sudo apt install bubblewrap slirp4netns`). The runtime uses `/usr/bin/bwrap` and defaults to `/usr/bin/slirp4netns`. For a helper installed elsewhere, set `TEAMAGENTS_SLIRP4NETNS` to its trusted absolute executable path before starting the daemon. The host must permit unprivileged user and network namespaces. The binary archive and source installer require these prerequisites to be installed separately.

To build from source, use the pinned Rust toolchain and a C compiler for bundled SQLite. If TeamAgents is already running, let in-flight work finish and run `teamagents stop` in each running session before replacing the binary. From the checkout, run `cargo install --locked --path .` or `./install.sh`. Add Cargo's installation directory (usually `~/.cargo/bin`) to `PATH` and run `teamagents version` to check the installation.

Choose a model provider before starting the session daemon. Run the client in the project directory you want to work on.

**ChatGPT subscription.** Run `teamagents auth login` and open the printed OpenAI URL in a browser on the same machine. Approve plan usage for TeamAgents and return to the terminal. Then run:

```sh
teamagents auth status
TEAMAGENTS_PROVIDER=chatgpt teamagents chat
```

The daemon uses OpenAI's [subscription Responses API](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference). `TEAMAGENTS_MODEL` sets the initial model. With no selection, the daemon chooses the first visible model for your account. All workers and the reporter share the selected provider. Account eligibility and usage limits are set by OpenAI. TeamAgents keeps local authorization in `~/.teamagents-auth/`, outside the workspace. Use `teamagents auth logout` to remove local authorization. If a project already has a running daemon, let active tasks settle and run `teamagents stop` before changing providers.

**API key (OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages).** Set `TEAMAGENTS_PROVIDER` to `api` (Chat Completions), `responses` (OpenAI Responses API), or `anthropic` (Anthropic Messages API):

```sh
# OpenAI Chat Completions (`api`) or OpenAI Responses (`responses`)
export TEAMAGENTS_PROVIDER=responses   # or: api
export OPENAI_BASE_URL=https://api.openai.com/v1  # default
export TEAMAGENTS_MODEL='your-provider-model-id'
# Set OPENAI_API_KEY in this environment using your secret manager.
teamagents chat

# Anthropic Messages (`anthropic`)
export TEAMAGENTS_PROVIDER=anthropic
export ANTHROPIC_BASE_URL=https://api.anthropic.com  # default
export TEAMAGENTS_MODEL='claude-sonnet-4-5'          # or ANTHROPIC_MODEL
# Set ANTHROPIC_API_KEY (and optional ANTHROPIC_MAX_TOKENS) via your secret manager.
teamagents chat
```

By default, the daemon selects subscription access when signed in, `anthropic` when only `ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL` are set, and `api` otherwise. All four providers expose native single-action tool schemas (with raw JSON text fallback) so models do not have to hand-format large escapes inside prose. Non-loopback API endpoints require HTTPS and a key (`OPENAI_API_KEY` or `ANTHROPIC_API_KEY`), while loopback endpoints may use HTTP without a key. For `anthropic`, reasoning efforts map to Anthropic `thinking` budgets (and `none` disables thinking). Model requests use a 600-second read timeout (30-second connect, 60-second write), follow up to 5 redirects, coordinate rate-limit waits across workers, and retry transient network/408/429/5xx/529 failures up to 5 attempts total; quota exhaustion triggers a 30-second circuit-breaker pause so queued branches settle cleanly until resumed with `continue`; HTTP 413 / context-overflow responses and oversized worker histories trigger automatic history compaction.

### Switch models and reasoning effort in chat

| Chat command | Effect |
|---|---|
| `/model` | Show the provider, model selection and reasoning effort for the next task. |
| `/model list` | Show model IDs visible to the signed-in ChatGPT account. For an API provider, use a model ID supplied by that provider. |
| `/model MODEL [EFFORT]` | Choose a model. Omitting the effort uses that model's default. Use `/model set MODEL [EFFORT]` for an ID such as `default` that also names a command. |
| `/model effort high` | Change only the effort. `/model effort default` restores the model's default. |
| `/model default` | Restore the daemon's `TEAMAGENTS_MODEL` setting and the model's default effort. |

TeamAgents accepts `none`, `minimal`, `low`, `medium`, `high`, `xhigh` and `max`. Each model supports its own subset. `default` lets the provider choose. The selection is saved in this session for the current provider and takes effect on the next `chat` or `exec` submission. Children, repair stages and the final reporter use the selection saved with their root, including when work resumes after clarification or a restart.

### Interactive TUI and plain chat

Running `teamagents` or `teamagents chat [--review] [--staged]` on an interactive terminal opens the TUI directly into a blank **New Session** (previous sessions stay accessible in the sidebar); piped input, `--json`, or `--plain` uses the line-oriented chat REPL (also starting in a fresh session). Use `teamagents tui [--review] [--staged]` to require the TUI explicitly. Shortcuts handle navigation, layout, item actions, and exit, while `/` commands configure workflows without overlap:

| Shortcut | Action (navigation, layout & direct item control) |
|---|---|
| `Tab` / `BackTab` | Cycle focus (`Composer`, `Sidebar`, `Inspector`) or complete `/` command |
| `Ctrl+N` | Switch to a blank New Session (or click `+New` / run `/new`) |
| `Ctrl+B` | Toggle sidebar rail |
| `Ctrl+P` | Open pending approvals queue (`a` approve, `d` deny) |
| `Alt+1`–`5` / `1`–`5` | Switch inspector tabs (`Summary`, `Evidence`, `Tree`, `Log`, `Board`) |
| `[` / `]` | Switch sidebar tabs (`Sessions`, `Agents`) |
| `a` / `d` / `r` / `x` | Approve / deny / reply clarification / cancel selected task |
| `Del` ×2 | Arm and confirm deleting the selected root session (idle/settled only) |
| `Enter` / `Shift+Enter` | Submit new prompt, continue settled leaf/summarizer, or insert newline |
| `F1` / `Ctrl+Q` | Open reference sheet / leave TUI (daemon keeps running) |

Complementary TUI slash commands: `/model`, `/to`, `/check`, `/review`, `/staged`, `/wb`, `/new [TEXT]`, `/answer ROOT_ID TEXT`, `/clear`, and `/stop`.

## Submit work and verify the result

By default, new tasks run unattended. `exec` waits for a final report or a clarification question, with no client deadline. This example asks the team to fix a script and defines how to check its output.

```sh
teamagents exec --check 'test "$(sh greet.sh Ada)" = "Hello, Ada!"' \
  'Fix greet.sh so it greets its argument by name.'
```

**How work moves, collaborates, and continues.** A `handoff` closes the current stage and creates 1–64 children in one transaction. Each child receives the sender's stage summary and its own assignment. Within a single round (`(root, round)`), each member takes at most one stage. When a round finishes with **1 terminal leaf**, it settles directly at that leaf; with **2+ terminal leaves**, it converges into an in-tree `summarizer` node (`kind = "summarizer"`) that also receives a read-only `whiteboard` snapshot (treated strictly as unverified collaborative notes) without workspace tools or private worker histories. Active sibling leaves can coordinate via the root's persistent collaborative whiteboard (`wb_read`, `wb_write` with CAS `expected_version` / lock-free `append` / `resolve` / `delete`, and deadlock-free `wb_wait`). Whiteboard entries persist across follow-up rounds until deleted with the root; inspect or edit them via `teamagents whiteboard ROOT_ID` (`wb`) or `/wb ...` in chat. Follow-up input (`teamagents continue TASK_ID TEXT` or plain composer text while viewing that settled session/node) sends the prompt to that sole leaf or summarizer node and promotes it to the root of the next subtree round with reusable session members and no turn-budget cap. Sibling or ancestor hops remain disallowed within a round.

**What counts as verified.** `--check` authorizes the exact shell command you supplied for the active round. Once execution leaves finish, a checked round must pass that command before settling (with 2+ leaves, before the summarizer writes the report). A failed check can start a new repair stage with the diagnostics. The limit is **ten check attempts total** per round. Without `--check`, even a successful model-requested shell command leaves `verification` set to `not_requested`.

**Follow a root or answer a question.** Use `exec --detach --json` to get the root ID immediately. `exec --timeout SECONDS` limits only the client's wait. To reconnect, use the same session and workspace. The first stage can ask for a missing decision before it uses tools or hands off. `exec` prints the question and exits with a nonzero status. Work resumes when you answer. For example, if the agent asks which greeting to implement, inspect the root and reply.

```sh
teamagents status ROOT_ID --json
teamagents answer ROOT_ID 'Use the long greeting.'
teamagents whiteboard ROOT_ID set contract:greeting 'Greet first argument by name.'
```

`teamagents status ROOT_ID` shows the whole run, including a tree outline with stage statuses, whiteboard state, and task IDs once it branches. In JSON, `status` is the aggregate outcome and `stage_status` tracks the first stage, which may already be `done`. `tree_nodes` lists the stages, `leaf_results` contains leaf statuses and reports, `whiteboard_seq` / `whiteboard` show collaborative entries, and `subtree_receipts` indexes observed operations. Inspect `teamagents status TASK_ID` for an individual stage's full `evidence`.

When root status is `input`, read `question` and reply to that root ID. In chat, use `/status ROOT_ID`, `/answer ROOT_ID TEXT`, and `/wb ...`. Plain text answers a waiting question on the selected session, or in a settled session continues the sole leaf/summarizer node (`continue TASK_ID TEXT`). Use `Ctrl+N` / `/new` to switch back to a blank session or `/new TEXT` to start separate work directly.

`/members` lists agents and `/to NAME` selects the recipient of the next chat submission. `/tasks` lists work in the session. Leaving chat or timing out keeps work running. `cancel TASK_ID` requests cancellation, `delete ROOT_ID` removes a settled or idle root tree (and its whiteboard entries), and `stop` closes an idle daemon while keeping the session.

**To review each model-requested shell command or edit**, submit with `teamagents exec --review '…'` or start `teamagents chat --review`. Children inherit the root's policy. In the same session, use `teamagents tasks` to find pending work, then `teamagents status TASK_ID` to read the proposed operation. Pass its `approval_id` to `teamagents approve APPROVAL_ID` or `teamagents deny APPROVAL_ID`. The exact `--check` command was authorized at submission.

## What the workspace can access

By default, shell commands and checks run in `bwrap` **direct workspace-write** mode bound directly to the project under `/workspace`, hiding `.teamagents-auth` and mounting top-level `.git` and Git admin directories read-only, with no staging copy size, entry count, or depth walls. The sandbox home and `/tmp` are private to each command. Network installs into those directories do not persist across commands.

Pass `--staged` (or toggle `/staged` in chat/TUI) to run in isolated staged copy-on-exit mode instead. Staging supports directories and single-link regular files up to 256 MiB, 32,768 entries and 64 directory levels (refusing symlinks, hard-linked files, special files and nested mounts while excluding Git metadata and Cargo `target` caches marked with `CACHEDIR.TAG`), and checks for host conflicts before publishing changes one entry at a time (even on nonzero exit; crashes partway through can leave partial writes).

Shell commands accept an optional model-specified `timeout_ms` (`100..=600_000`, default `60s`; timeouts report `exit 124` and partial output) and apply middle truncation to stdout/stderr at 128 KiB per stream (`64 KiB` head + `64 KiB` tail) without aborting on large output. `edit_file` handles files up to 8 MiB, while `read_file` defaults to an 8 MiB slice and supports `offset`/`limit` pagination beyond 8 MiB.

**Choose the workspace with care.** Shells have outbound network access and can send workspace files, including any secrets you placed there, to external services. The sandbox does not mount the host home, credential stores or the daemon socket. Loopback belongs to the sandbox, but other services on your LAN may still be reachable.

## Sessions and runtime bounds

`--workspace DIR` selects the project (default: current directory). Its canonical path selects a session under `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents-swarm/sessions/<workspace-hash>`. `--session DIR` overrides that location. Use the same pair of paths to reconnect. The session directory must be owned by you, must not be group- or world-writable, and cannot overlap the workspace.

| Runtime bound | Value | Scope |
|---|---:|---|
| Model turns / members / concurrency | Unlimited | No turn budget, member count cap, concurrency cap, or same-agent serialization |
| Handoff / check / format retries | 64 / 10 / 5 | Max handoff children / max `--check` attempts / consecutive worker protocol retries (resets on accepted action) |
| File read (`read_file`) & edit (`edit_file`) | 8 MiB (paginated read) | `edit_file` limit; `read_file` page size with optional `offset`/`limit` |
| Shell timeout & output / IPC frame | 60s (max 600s) / 128 KiB / 8 MiB | Configurable `timeout_ms` / middle-truncated per-stream cap / IPC frame cap |
| Model HTTP / payload / SSE stream | 600s / 2 MiB (12 MiB reporter) / 32 MiB | HTTP read timeout / request, response & action text / ChatGPT SSE stream limit |
| Auto-compaction & whiteboard digest | 384 KiB (>=6 msgs) / 4 KiB | Soft auto-compaction trigger (plus HTTP 413/overflow) / transient board digest |
| Summarizer input / TUI events | 2 MiB / 8 MiB / 1000 | Leaf outputs limit / intent & clarifications limit / retained TUI events |

One daemon writes each session's SQLite database with WAL and `synchronous=FULL`. If a started tool lacks a durable result, its outcome is unknown. Inspect the workspace and any external effects before starting new work. The daemon never replays an unknown operation. Cancellation cannot undo effects or prove that a command stopped. Call counts and concurrency do not bound token charges.

For development, `make build` builds `target/debug/teamagents`, `make fmt` formats the crate, and `make check` runs format checks, Clippy and tests. `make package` writes a host binary archive and `SHA256SUMS` under `target/dist/`.
