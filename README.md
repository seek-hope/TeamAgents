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

The daemon uses OpenAI's [subscription Responses API](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference). `TEAMAGENTS_MODEL` sets the initial model. With no selection, the daemon chooses the first visible model for your account. All workers and the reporter share the root's selected provider. Account eligibility and usage limits are set by OpenAI. TeamAgents keeps local authorization in `~/.teamagents-auth/`, outside the workspace. Use `teamagents auth logout` to remove local authorization. Inside an interactive session (`chat` / TUI), you can switch providers live without stopping the daemon via `/model provider NAME [MODEL [EFFORT]]` (or the `Provider:` bar in the `/model` dialog); standalone `TEAMAGENTS_PROVIDER=... teamagents exec` still refuses mismatches unless acknowledged or cleared with `teamagents stop` for billing safety.

**Custom providers (`providers.json` & `teamagents provider`).** You can register additional OpenAI Responses, OpenAI Chat Completions, or Anthropic Messages endpoints in `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/providers.json` (`TEAMAGENTS_PROVIDERS_FILE` overrides the path). Configuration files store only credential **environment variable names** (`--api-key-env VAR`), never raw API keys:

```sh
teamagents provider add openrouter --wire chat_completions \
  --base-url https://openrouter.ai/api/v1 \
  --api-key-env OPENROUTER_API_KEY --model openai/gpt-4.1-mini
teamagents provider list                 # inspect configured + built-in readiness
teamagents provider show openrouter      # show redacted endpoint settings
teamagents provider default openrouter   # set global default (or `default auto`)
teamagents provider remove openrouter    # remove a custom entry
```

**API key (DeepSeek Responses, OpenAI Chat Completions, OpenAI Responses, or Anthropic Messages).** Set `TEAMAGENTS_PROVIDER` to `deepseek` (DeepSeek Responses API), `api` (Chat Completions), `responses` (OpenAI Responses API), or `anthropic` (Anthropic Messages API):

```sh
# DeepSeek Responses (`deepseek`, auto-selected first when DEEPSEEK_API_KEY is set)
# Default model: deepseek-flash; default base URL: https://api.deepseek.com/v1
# Set DEEPSEEK_API_KEY in this environment using your secret manager.
teamagents chat

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

By default (when `TEAMAGENTS_PROVIDER` is unset), the daemon selects `deepseek` whenever `DEEPSEEK_API_KEY` is set (defaulting to `deepseek-flash` over `/v1/responses`), else ChatGPT subscription access when signed in, else `anthropic` when only `ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL` are set, and `api` otherwise. All five providers share a unified static tool schema and identity-last prompt layout, fork parent conversation history by default across `handoff` (`inherit_context: true`, opt-out via `false`), replay native reasoning and cache hints (`prompt_cache_key`, Anthropic `cache_control`), and report per-node and root cache-hit telemetry (`usage`). Non-loopback API endpoints require HTTPS and a key (`DEEPSEEK_API_KEY`, `OPENAI_API_KEY`, or `ANTHROPIC_API_KEY`), while loopback endpoints may use HTTP without a key. For `anthropic`, reasoning efforts map to Anthropic `thinking` budgets (and `none` disables thinking). Model requests use a 600-second read timeout (30-second connect, 60-second write), follow up to 5 redirects, coordinate rate-limit waits across workers, and retry transient network/408/429/500/502/503/504/529 failures up to 5 attempts total; quota exhaustion triggers a 30-second circuit-breaker pause so queued branches settle cleanly until resumed with `continue`; HTTP 413 / context-overflow responses and worker histories estimated at 90% of the model context window (`TEAMAGENTS_CONTEXT_WINDOW` override available) trigger automatic history compaction. The 90% figure is a byte-based estimate plus the provider's last input-token count, not a tokenizer measurement; the 8 MiB request ceiling is enforced separately.

### Switch provider, model and reasoning effort in chat

| Chat command | Effect |
|---|---|
| `/model` | Show the provider, model selection and reasoning effort for the next task (or open the `/model` selector in TUI). |
| `/model providers` / `/provider` | List available built-in and custom providers with readiness status. |
| `/model provider NAME [MODEL [EFFORT]]` | Switch the session's active provider live without restarting the daemon (`/model provider default` clears the session override; alias `/provider use NAME`). |
| `/provider add \| show \| remove \| default` | Add, inspect, remove, or set default custom providers inside chat/TUI. |
| `/model list` | Query visible model IDs from the active provider (ChatGPT catalog or `/models` API endpoint). |
| `/model MODEL [EFFORT]` | Choose a model for the active provider. Omitting the effort uses that model's default. Use `/model set MODEL [EFFORT]` for an ID such as `default` that also names a command. |
| `/model effort high` | Change only the effort. `/model effort default` restores the model's default. |
| `/model default` | Restore the active provider's default model and reasoning effort. |

TeamAgents accepts `none`, `minimal`, `low`, `medium`, `high`, `xhigh` and `max`. Each model supports its own subset. `default` lets the provider choose. Provider choice and model/effort overrides are saved per session (and per provider) and take effect on the next `chat` or `exec` submission. In-flight or settled roots keep the provider, model and effort snapshot captured at submission for all children, repair stages, and the final reporter.

### Interactive TUI and plain chat

Running `teamagents` or `teamagents chat [--review] [--staged|--sandbox MODE]` on an interactive terminal opens the TUI directly into a blank **New Session** (previous sessions stay accessible in the sidebar); piped input, `--json`, or `--plain` uses the line-oriented chat REPL (also starting in a fresh session). Use `teamagents tui [--review] [--staged|--sandbox MODE]` (or `teamagents resume [TASK_ID]`) to open or resume in the TUI. While a non-legacy root session is running in the TUI, typing plain text in the composer queues mid-turn steering guidance (`steer`), and typing `@` opens fuzzy workspace file completion (`Tab`/`Enter` inserts `@path`). Live reasoning/commentary, active tools, streaming shell tails, and `update_plan` checklists update continuously in the Summary inspector. Shortcuts handle navigation, layout, item actions, and exit, while `/` commands configure workflows without overlap:

| Shortcut | Action (navigation, layout & direct item control) |
|---|---|
| `Tab` / `BackTab` | Complete `/` command or `@file`, or cycle focus (`Composer`, `Sidebar`, `Inspector`) |
| `Ctrl+N` | Switch to a blank New Session (or click `+New` / run `/new`) |
| `Ctrl+B` | Toggle sidebar rail |
| `Ctrl+P` | Open pending approvals queue (`a` approve, `d` deny) |
| `Alt+1`–`5` / `1`–`5` | Switch inspector tabs (`Summary`, `Evidence`, `Tree`, `Log`, `Board`) |
| `[` / `]` | Switch sidebar tabs (`Sessions`, `Agents`) |
| `a` / `d` / `r` / `x` | Approve / deny / reply clarification / cancel selected task |
| `Del` ×2 | Arm and confirm deleting the selected root session (idle/settled only) |
| `Enter` / `Shift+Enter` | Submit prompt, steer running root, continue settled leaf/summarizer, or insert newline |
| `F1` / `Ctrl+Q` | Open reference sheet / leave TUI (daemon keeps running) |

Shared TUI/chat commands: `/model`, `/provider`, `/to`, `/review code [REF]`, `/sandbox [direct|staged|read-only]`, `/staged [on|off]`, `/plan TEXT`, `/diff [REF]`, `/steer TEXT`, `/compact`, `/skills`, `/mcp`, `/status`, `/wb`, `/new [TEXT]`, and `/answer ROOT_ID TEXT`. TUI-only: `/check`, `/review [on|off]`, `/clear`, and `/stop`. The line-oriented chat client has no `/check`, `/clear`, or `/stop` command.

## Workspace instructions, skills, and MCP

- **Project instructions (`AGENTS.md`).** TeamAgents discovers hierarchical `AGENTS.override.md` and `AGENTS.md` files from repository root to the workspace, plus `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/AGENTS.md` or `~/.teamagents/AGENTS.md`, and injects them along with git/environment context into root tasks.
- **Local skills (`SKILL.md` & `read_skill`).** Place reusable workflows under `.teamagents/skills/`, `.agents/skills/`, `~/.teamagents/skills/`, or `~/.agents/skills/` as `<skill>/SKILL.md` with YAML frontmatter (`name`, `description`). Roots see the discovered skill index and can load full instructions on demand with `read_skill` (or inspect them via `teamagents skills` and `/skills`).
- **MCP servers (`mcp.json` & `teamagents mcp`).** Register stdio or Streamable HTTP JSON-RPC 2.0 MCP servers in `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/mcp.json` (`TEAMAGENTS_MCP_FILE` overrides the path), storing only credential environment variable names (`--env VAR`, `--bearer-token-env VAR`, never raw secrets):

```sh
teamagents mcp add docs --command npx --arg -y --arg @modelcontextprotocol/server-filesystem --arg .
teamagents mcp add remote --url https://mcp.example.com/mcp --bearer-token-env MCP_TOKEN
teamagents mcp list
teamagents mcp tools [NAME]
```

Workers inspect configured MCP tools via `mcp_list` and invoke them with `mcp_call`. MCP servers run as host processes outside `bwrap`; the daemon clears their environment and does not forward `HOME` unless you explicitly list it in `env_vars`. For that reason `mcp_list`/`mcp_call` are available only in direct sandbox mode, not staged or read-only mode.

## Submit work and verify the result

By default, new tasks run unattended. `exec` waits for a final report or a clarification question, with no client deadline; pass `--jsonl` to stream real-time `submitted`, `live`, `plan`, `input`, `approval`, and `completed` JSONL events, `review [--uncommitted|--base REF|--commit SHA]` for read-only code review, `resume [TASK_ID [TEXT]]` to reconnect or extend a session, `diff [--base REF|--commit SHA]` to inspect git diffs, `steer TASK_ID TEXT` to guide an in-flight run, and `compact TASK_ID` to compact worker conversation history on demand. This example asks the team to fix a script and defines how to check its output.

```sh
teamagents exec --check 'test "$(sh greet.sh Ada)" = "Hello, Ada!"' \
  'Fix greet.sh so it greets its argument by name.'
```

**How work moves, collaborates, and continues.** Active `Root` and `Node` stages can batch up to 16 native tool calls in a single model turn (`read_file` with optional `line_numbers`, `list_dir`, `grep_files`, `read_skill`, `mcp_list`, `mcp_call`, V4A `apply_patch`, `edit_file`, `update_plan`, `shell`, `wb_read`, `wb_write`), running read-only tools concurrently and side-effecting ops sequentially. A `handoff` closes the current stage and creates 1–64 children in one transaction. Each child receives the sender's stage summary and its own assignment. Within a single round (`(root, round)`), each member takes at most one stage. When a round finishes with **1 terminal leaf**, it settles directly at that leaf; with **2+ terminal leaves**, it converges into an in-tree `summarizer` node (`kind = "summarizer"`) that also receives a read-only `whiteboard` snapshot (treated strictly as unverified collaborative notes) without workspace tools or private worker histories. Active sibling leaves can coordinate via the root's persistent collaborative whiteboard (`wb_read`, `wb_write` with CAS `expected_version` and single-writer serialized `append` / `resolve` / `delete`, and `wb_wait`). `wb_wait` wakes on a matching write/delete or when no non-waiting writers remain; a sibling parked on human approval can keep a keyed waiter asleep until that approval is resolved. Whiteboard entries persist across follow-up rounds until deleted with the root; inspect or edit them via `teamagents whiteboard ROOT_ID` (`wb`) or `/wb ...` in chat. Follow-up input (`teamagents continue TASK_ID TEXT`, `teamagents resume TASK_ID TEXT`, or plain composer text while viewing that settled session/node) sends the prompt to that sole leaf or summarizer node and promotes it to the root of the next subtree round with reusable session members and no turn-budget cap. Sibling or ancestor hops remain disallowed within a round.

A batched `wb_read` that exceeds the result budget returns whole entries in ascending `seq` order, with `truncated: true` and `omitted_entries`. The unread watermark stays at its prior value. To fetch more, pass the last returned entry's `seq` as `since_seq`. If the result contains no entries, narrow the read with `key` or `prefix`.

**What counts as verified.** `--check` authorizes the exact shell command you supplied for the active round. Once execution leaves finish, a checked round must pass that command before settling (with 2+ leaves, before the summarizer writes the report). A failed check can start a new repair stage with the diagnostics. The limit is **ten check attempts total** per round. Without `--check`, even a successful model-requested shell command leaves `verification` set to `not_requested`.

**Follow a root or answer a question.** Use `exec --detach --json` to get the root ID immediately. `exec --timeout SECONDS` limits only the client's wait. To reconnect, use the same session and workspace. `resume` without an ID selects the newest root. `resume ROOT_ID` opens that root in the TUI on an interactive terminal. Pass `--json`, `--jsonl`, `--timeout SECONDS` or `--detach` to select the command-line client there. The first stage can ask for a missing decision before it uses tools or hands off. `exec` prints the question and exits with a nonzero status. Work resumes when you answer. For example, if the agent asks which greeting to implement, inspect the root and reply.

```sh
teamagents status ROOT_ID --json
teamagents answer ROOT_ID 'Use the long greeting.'
teamagents whiteboard ROOT_ID set contract:greeting 'Greet first argument by name.'
```

`teamagents status ROOT_ID` shows the whole run, including a tree outline with stage statuses, whiteboard state, and task IDs once it branches. In JSON, `status` is the aggregate outcome and `stage_status` tracks the first stage, which may already be `done`. `tree_nodes` lists the stages, `leaf_results` contains leaf statuses and reports, `whiteboard_seq` / `whiteboard` show collaborative entries, and `subtree_receipts` indexes observed operations. Inspect `teamagents status TASK_ID` for an individual stage's full `evidence`.

When root status is `input`, read `question` and reply to that root ID. In chat, use `/status ROOT_ID`, `/answer ROOT_ID TEXT`, and `/wb ...`. Plain text answers a waiting question on the selected session, or in a settled session continues the sole leaf/summarizer node (`continue TASK_ID TEXT`). Use `Ctrl+N` / `/new` to switch back to a blank session or `/new TEXT` to start separate work directly.

`/members` lists agents and `/to NAME` selects the recipient of the next chat submission. `/tasks` lists work in the session. Leaving chat or timing out keeps work running. `cancel TASK_ID` requests cancellation; `delete ROOT_ID` removes a settled root tree (and its whiteboard entries) and refuses while a queued/running/waiting/approval stage, model turn, tool operation, or reporter is active. `stop` closes an idle daemon while keeping the session.

**To review each model-requested shell command or edit**, submit with `teamagents exec --review '…'` or start `teamagents chat --review`. Children inherit the root's policy. In the same session, use `teamagents tasks` to find pending work, then `teamagents status TASK_ID` to read the proposed operation. Pass its `approval_id` to `teamagents approve APPROVAL_ID` or `teamagents deny APPROVAL_ID`. The exact `--check` command was authorized at submission.

## What the workspace can access

By default, shell commands and checks run in `bwrap` **direct workspace-write** mode bound directly to the project under `/workspace`, hiding the top-level `.teamagents-auth` entry and mounting top-level `.git` and Git admin directories read-only (nested copies inside the workspace are not specially hidden by direct shell mounts; native file tools reject them recursively), with no staging copy size, entry count, or depth walls. The sandbox home and `/tmp` are private to each command. Network installs into those directories do not persist across commands.

Pass `--staged` / `--sandbox staged` (or `/sandbox staged` / `/staged` in chat/TUI) to run in isolated staged copy-on-exit mode instead, or `--sandbox read-only` (`/sandbox read-only`) to mount `/workspace` read-only in `bwrap` and disable `edit_file`/`apply_patch` while still allowing read inspection. Staging supports directories and single-link regular files up to 256 MiB, 32,768 entries and 64 directory levels (refusing symlinks, hard-linked files, special files and nested mounts). Git metadata and Cargo `target` caches marked with `CACHEDIR.TAG` are skipped during publication rather than rejecting the whole publish. It compares host conflicts from one snapshot before writeback and rechecks each entry immediately before publishing (even on nonzero exit; external writers need not honor the workspace flock, and crashes partway through can leave partial writes). Native `edit_file`/`apply_patch` write the host workspace directly and do not use the staged copy; only shell/check commands stage.

Shell commands accept an optional model-specified `timeout_ms` (`100..=600_000`, default `60s`; timeouts report `exit 124` and partial output) and apply middle truncation to stdout/stderr at 128 KiB per stream (`64 KiB` head + `64 KiB` tail) without aborting on large output. `edit_file` and `apply_patch` handle files up to 8 MiB. `read_file` reads at most 8 MiB per call; a larger file requires `offset`/`limit` (or `line_numbers`) pagination, otherwise the tool returns an error. To protect the model request, any single tool result is stored truncated at 1 MiB with an explicit marker.

**Choose the workspace with care.** Shells have outbound network access and can send workspace files, including any secrets you placed there, to external services. The sandbox does not mount the host home, credential stores or the daemon socket. Loopback belongs to the sandbox, but other services on your LAN may still be reachable.

## Sessions and runtime bounds

`--workspace DIR` selects the project (default: current directory). Its canonical path selects a session under `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents-swarm/sessions/<workspace-hash>`. `--session DIR` overrides that location. Use the same pair of paths to reconnect. The session directory must be owned by you, must not be group- or world-writable, and cannot overlap the workspace.

| Runtime bound | Value | Scope |
|---|---:|---|
| Model turns / members / concurrency | Unlimited | No turn budget, member count cap, concurrency cap, or same-agent serialization |
| Handoff / batch / check / format retries | 64 / 16 / 10 / 5 | Max handoff children / tool calls per parallel turn / `--check` attempts / worker protocol retries |
| File read (`read_file`) & edit / patch | 8 MiB (paginated read) | `edit_file` & `apply_patch` limit; `read_file` page size with optional `line_numbers` & `offset`/`limit` |
| Shell timeout & output / IPC frame | 60s (max 600s) / 128 KiB / 8 MiB | Configurable `timeout_ms` / middle-truncated per-stream cap / IPC frame cap |
| Model HTTP / payload / SSE stream | 600s / 8 MiB worker request, 2 MiB response; 12 MiB reporter request / 32 MiB | HTTP read timeout / request cap & response/action text / SSE stream limit |
| Tool result / auto-compaction / whiteboard digest | 1 MiB / estimated 90% window (>=4 persisted messages) / 4 KiB | Stored per-result truncation / byte-estimate + `TEAMAGENTS_CONTEXT_WINDOW` / newest-first unread digest |
| Summarizer inputs / TUI / MCP | 2 MiB leaves, 8 MiB intent / 1000 events / 30s | Leaf results / intent limit / retained TUI events / MCP `256 KiB` config, `30s` RPC, `2 MiB` response |

One daemon writes each session's SQLite database with WAL and `synchronous=FULL`. If a started tool lacks a durable result, its outcome is unknown. Inspect the workspace and any external effects before starting new work. The daemon never replays an unknown operation. Cancellation cannot undo effects or prove that a command stopped. Call counts and concurrency do not bound token charges.

For development, `make build` builds `target/debug/teamagents`, `make fmt` formats the crate, and `make check` runs format checks, Clippy and tests. `make package` writes a host binary archive and `SHA256SUMS` under `target/dist/`.

### Testing and upgrading TeamAgents with TeamAgents

1. Let the running version edit a candidate checkout. Use workspace-relative paths for native tools and `/workspace` for shell commands. Keep Git operations that write metadata in the maintainer's checkout. The sandbox gives each shell a private `CARGO_HOME` and `CARGO_TARGET_DIR`, so run related build and test steps in one invocation to reuse their outputs.
2. Verify that exact candidate revision or working-tree snapshot on a disposable Linux runner with working user and network namespaces. `--check 'make check'` executes inside the root's sandbox. If nested `bwrap` cannot start there, record the failure and run the full `make check` on the runner with the pinned toolchain and loopback test providers. Keep the runner's result alongside the candidate digest. For repeated builds, the runner can reuse a dedicated dependency cache keyed by toolchain and `Cargo.lock` with a fresh build directory for each candidate.
3. Try the candidate binary in a fresh workspace and session. Check a verified task, restart the candidate daemon, and resume the task. Before installing, drain and stop the old daemons and retain the previous binary and backups of the stopped session directories. If the candidate fails the trial, keep the existing version. If a later rollback needs an older database schema, restore the matching session backup along with the binary.
