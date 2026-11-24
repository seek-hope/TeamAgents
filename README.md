# TeamAgents

[中文](README.zh-CN.md) · [Design decisions](docs/DECISIONS.md) · [0.5.0 architecture](docs/teamagents-0.5.0.canvas)

TeamAgents 0.5.0 runs coding tasks in a local workspace. A new task starts with one agent, `default` unless you select another. A stage can do the work itself or hand off to new child stages. Work flows down the tree. When the leaves finish, a summarizer outside the tree prepares the report for the user.

## Install and start

The shell sandbox supports native x86-64 and little-endian AArch64 Linux. Install `bubblewrap` and `slirp4netns` with your distribution's package manager (on Ubuntu, `sudo apt install bubblewrap slirp4netns`). The runtime uses `/usr/bin/bwrap` and defaults to `/usr/bin/slirp4netns`. For a helper installed elsewhere, set `TEAMAGENTS_SLIRP4NETNS` to its trusted absolute executable path before starting the daemon. The host must permit unprivileged user and network namespaces. The binary archive and source installer require these prerequisites to be installed separately.

Before replacing the binary, let in-flight work settle and run `teamagents stop` in each running session with its matching client. Session migration preserves each root's routing, approval policy and pending operations. Schema 1 roots keep per-operation review. New submissions in the same session use the one-way tree.

To build from source, use the pinned Rust toolchain and a C compiler for bundled SQLite. From the checkout, run `cargo install --locked --path .` or `./install.sh`. Put Cargo's installation directory (usually `~/.cargo/bin`) on `PATH` and check `teamagents version` for `0.5.0`.

Set the provider environment before the session daemon starts. Then run the client in the project directory you want it to use.

```sh
export OPENAI_BASE_URL=https://api.openai.com/v1  # default
export TEAMAGENTS_MODEL='your-provider-model-id'
# Set OPENAI_API_KEY in this environment using your secret manager.
teamagents chat
```

The daemon sends non-streaming Chat Completions to the configured provider, separately from the shell network. Non-loopback endpoints require HTTPS and `OPENAI_API_KEY`. A loopback provider may use HTTP without a key. Requests time out after 120 seconds and do not follow redirects or retry HTTP errors.

## Submit work and verify the result

By default, new tasks run unattended. `exec` waits for a final report or a clarification question, with no client deadline. This example asks the team to fix a script and defines how to check its output.

```sh
teamagents exec --check 'test "$(sh greet.sh Ada)" = "Hello, Ada!"' \
  'Fix greet.sh so it greets its argument by name.'
```

**How work moves.** A `handoff` closes the current stage and creates 1–16 children in one transaction. Each child receives the sender's stage summary and its own assignment. The handoff can add new named workers up to the session's member limit, or use existing members who have not yet worked on this root. Each member takes at most one stage per tree. Children can use workspace tools, finish as leaves, or hand off again. No child can wake its parent or send work to a sibling.

**What counts as verified.** `--check` authorizes the exact shell command you supplied. Once the execution leaves have finished, a checked root must pass that command before the summarizer can issue a successful report. A failed check can start a new repair stage with the diagnostics, even when the session's member slots are full. The limit is **three check attempts total**, including the first, and the shared model-call budget can run out sooner. Without `--check`, even a successful model-requested shell command leaves `verification` set to `not_requested`.

The summarizer receives the original request, any clarifications, leaf results and the runtime's outcome and verification status. Private agent histories and raw tool output stay outside its input. It cannot run tools, reopen stages or change verification. If it cannot produce a usable report, the runtime returns a partial report from recorded leaves and marks an otherwise successful run `failed`.

**Follow a root or answer a question.** Use `exec --detach --json` to get the root ID immediately. `exec --timeout SECONDS` limits only the client's wait. To reconnect, use the same session and workspace. The first stage can ask for a missing decision before it uses tools or hands off. `exec` prints the question and exits with a nonzero status. Work resumes when you answer. For example, if the agent asks which greeting to implement, inspect the root and reply.

```sh
teamagents status ROOT_ID --json
teamagents answer ROOT_ID 'Use the long greeting.'
```

`teamagents status ROOT_ID` shows the whole run, including a tree outline with stage statuses and task IDs once it branches. In JSON, `status` is the aggregate outcome and `stage_status` tracks the first stage, which may already be `done`. `tree_nodes` lists the stages, `leaf_results` contains leaf statuses and reports, and `subtree_receipts` indexes observed operations. Inspect `teamagents status TASK_ID` for an individual stage's full `evidence`.

When root status is `input`, read `question` and reply to that root ID. In chat, use `/status ROOT_ID` and `/answer ROOT_ID TEXT`. Plain text answers a question when it is the only pending question and belongs to the selected agent. Use `/new TEXT` to start a separate task while a question is open.

`/members` lists agents and `/to NAME` selects the recipient of the next chat submission. `/tasks` lists work in the session. Leaving chat or timing out keeps work running. `cancel TASK_ID` requests cancellation. `stop` closes an idle daemon and keeps the session.

**To review each model-requested shell command or edit**, submit with `teamagents exec --review '…'` or start `teamagents chat --review`. Children inherit the root's policy. In the same session, use `teamagents tasks` to find pending work, then `teamagents status TASK_ID` to read the proposed operation. Pass its `approval_id` to `teamagents approve APPROVAL_ID` or `teamagents deny APPROVAL_ID`. The exact `--check` command was authorized at submission.

## What the workspace can access

Shell commands and checks work under `/workspace`, a staged copy. The sandbox home and `/tmp` are private to each command. Network installs into those directories, including Cargo caches, do not persist across commands. Staging supports directories and single-link regular files, up to 256 MiB, 32,768 entries and 64 directory levels. Symlinks, hard-linked files, special files and nested mounts are refused. Git metadata and Cargo `target` caches marked with `CACHEDIR.TAG` are excluded. Use a smaller project directory if staging hits a limit.

A completed command can publish changes even if its exit code is nonzero. The runtime checks for conflicts with host edits before applying the changes one entry at a time. A crash or I/O error partway through publication can leave partial changes. Commands have a 60-second runtime limit and a 64 KiB limit per output stream.

**Choose the workspace with care.** Shells have outbound network access and can send workspace files, including any secrets you placed there, to external services. The sandbox does not mount the host home, credential stores or the daemon socket. Loopback belongs to the sandbox, but other services on your LAN may still be reachable.

## Sessions and limits

`--workspace DIR` selects the project (default: current directory). Its canonical path selects a session under `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents-swarm/sessions/<workspace-hash>`. `--session DIR` overrides that location. Use the same pair of paths to reconnect. The session directory must be owned by you, must not be group- or world-writable, and cannot overlap the workspace.

| Daemon setting | Default | Limit |
|---|---:|---|
| `TEAMAGENTS_MAX_AGENTS` | 8 | Persistent members, including `default`. Set at session creation. |
| `TEAMAGENTS_TURN_BUDGET` | 32 | Model calls for the whole root, including execution, repair and summarization. Set at session creation. A budget greater than one reserves one call for the report. |
| `TEAMAGENTS_CONCURRENCY` | 4 | In-flight model and tool work across the session. Set at daemon startup. |

One daemon writes each session's SQLite database with WAL and `synchronous=FULL`. If a started tool lacks a durable result, its outcome is unknown. Inspect the workspace and any external effects before starting new work. The daemon never replays an unknown operation. Cancellation cannot undo effects or prove that a command stopped. Call counts and concurrency do not bound token charges.

For development, `make build` builds `target/debug/teamagents`, `make fmt` formats the crate, and `make check` runs format checks, Clippy and tests. `make package` writes a host binary archive and `SHA256SUMS` under `target/dist/`.
