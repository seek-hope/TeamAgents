# How this product compares with Codex CLI, Pi and Hermes

Snapshot taken **2026-09-30** for the product direction the user set ("reference Codex CLI, pi and hermes").
It exists to make the *decisions* explicit, not to declare parity: every row says where its fact comes from,
and the last section lists what follows for this repository. It is a **surface comparison, not a quality or
capability benchmark** — §4 says what that excludes, and no comparator has been benchmarked here.

The first version was dated 2026-09-26/27. This refresh re-derived Pi and Hermes from their upstream (the Pi
row changed: it has MCP now) and re-read the TeamAgents column against this tree after D-362…D-371. The Codex
column was **not** re-derived this time (there is no `codex` binary on the machine that did the refresh), so it
keeps the 2026-09-27 check named below.

**Sources, and how strong they are**

| Reference | Source | Strength |
|---|---|---|
| Codex CLI | the installed binary, `codex-cli 0.156.1` (`codex --help`, `codex exec --help`, `codex mcp --help`, `codex sandbox --help`, `codex debug --help`, `codex app-server --help`) | **re-checked 2026-09-27 by `review/codex_surface.py`** (run by hand — it needs the local binary): every command/flag token the Codex column claims appears in those six help outputs. Not re-derived on 2026-09-30 (no binary present); an upgraded Codex CLI is a finding until the row is re-dated |
| Pi | the upstream README, its docs index (`packages/coding-agent/docs/docs.json`) and its file tree (GitHub API, `truncated:false`) | **re-derived 2026-09-30 by `review/comparison_sources.py`**: the tree holds **65** paths containing `mcp` (`packages/coding-agent/docs/mcp.md`, `src/extensions/mcp/`, `packages/mcp`), the docs index lists "Connect MCP Servers", and **no** path contains `worktree`; the subagent example still says "max 8, 4 concurrent" and "a separate `pi` process". The tree held 2,376 paths on that date (2,168 on 2026-09-26) — it grows, so the count is a snapshot and the script reports the drift rather than chasing it |
| Hermes | the upstream README (`NousResearch/hermes-agent`) plus the features/tools page of its docs site | **re-derived 2026-09-30 by `review/comparison_sources.py`**: the seven backends, cron, the TUI's interrupt-and-redirect, MCP toolsets, memory providers, session search and trajectory export are all still named there |
| TeamAgents | this repository: `docs/USER-GUIDE.md`, `docs/DECISIONS.md`, `docs/ACCEPTANCE.md`, `review/dogfood/*` | the evidence in this tree, re-read 2026-09-30 |

The two scripts above are the re-runnable halves of this table: `review/codex_surface.py` asks the installed
binary, and `review/comparison_sources.py` re-derives the upstream halves from the network. Both are run by
hand (a local binary, the network) and both are listed in the hygiene catalogue for that reason; the dates in
the table are theirs.

Pi and Hermes are documented here from their READMEs and docs — they were **not** run, so their rows describe
what their projects claim. Codex was run on 2026-09-27 (its help output is the evidence above).

## 1. By dimension

| Dimension | Codex CLI | Pi | Hermes | TeamAgents (2026-09-30) |
|---|---|---|---|---|
| Session / resume | `resume` (picker / `--last`), `fork`, `archive`, `delete`, `migrate-rollouts`, and `agents` to browse sessions on a *shared local app-server daemon* | resumable sessions, session history | conversation continuity across platforms, platform gateway | named sessions under one base (D-364/D-365/D-368): a registry at `<base>/sessions.json` and `teamagents sessions list`/`new`/`archive`/`delete`/`rename`/`restore`/`fork`/`search`, with `--session ID`; the base directory itself is the default session (A33), owned by one daemon the TUI and `exec` attach to. `fork` copies a session read-only (`VACUUM INTO`, D-365) and `search` reads across sessions read-only; there is **no interactive picker** in the TUI and no shared app-server across sessions |
| Permissions | config + `-c` overrides, `sandbox` subcommand | **none built in** ("runs with the permissions of the user and process that launched it"; containerize it for boundaries) | platform/sandbox backends | `approved_scope` (bubblewrap, or a locally present Docker image since D-369, approvals bound to the operation and its argument hash, D-67) or `full_auto` (host shell, D-41); capability grants with a user-facing surface (`teamagents authority`, D-61); the mode and the backend both come from the user config (D-75/D-369) |
| Sandbox backends | Linux sandbox + config | micro-VM / Docker / OpenShell patterns | seven terminal backends (local, Docker, SSH, Singularity, Modal, Daytona, Vercel Sandbox) | bubblewrap (default) or Docker (`[permissions] sandbox`/`sandbox_image`, D-369, refusing an image that is not present locally), for every command the product *executes*: the shell tool — which also runs the `[[checks]]`, the client's `--check` and the job runner's commands, all from one spec builder (`tools::shell_command_spec` → `tools::bwrap_argv` / `tools::docker_argv`) — and MCP servers with `mcp_execution = "workspace"` (the default). `mcp_execution = "host"` and `full_auto` are the explicit opt-outs, and a workspace MCP server under Docker is refused at config load. A missing or unusable backend is a classified failure, never a silent host fallback (A14); the choice is modelled by `V2Isolation` (`RunsOnlyUnderTheConfiguredBackend`, `UnavailableBackendNeverRuns`). Micro-VM/remote backends are **not** implemented |
| Tools | files, shell, apply (`codex apply`), review | files, shell, `!` commands, extensions | files, shell, scripts calling tools over RPC | files, shell (isolated or host), web (fetch/search — offered only for the kinds the config *declares*, D-168, and search needs a credential, D-167), MCP, skills; every tool call is an operation with a durable receipt (A08/A30) |
| Extensions | `mcp add/remove/login`, `plugin`, `features` | skills, prompt templates and extensions, **and MCP over stdio and streamable HTTP** (`pi mcp add`/`list`, an in-session `/mcp`, OAuth, resources, tool exposure) — upstream `packages/coding-agent/docs/mcp.md` and `src/extensions/mcp/`, present on 2026-09-30 and absent from the 2026-09-26 snapshot | MCP, skills (agentskills.io compatible), memory providers | MCP over stdio + streamable HTTP, declared in `[tools.*]` (D-74); skills from `~/.agents/skills` and configured roots (A26, D-34/D-66); user hooks `pre_tool`/`notify` (D-45, live evidence D-92). Pi's MCP management verbs are mostly matched now: `teamagents mcp list`/`add`/`remove` (D-399, D-408) show and edit the bindings, while `login` (OAuth) and an in-session `/mcp` remain absent (D-74/D-78) |
| Teams / subagents | (single agent per session; `fork` for branches) | one subagent extension example (one process per subagent, isolated context windows; ≤8 tasks, 4 concurrent); **no worktree isolation** — no path in the upstream tree contains "worktree" (still true on 2026-09-30) | subagents for parallel workstreams | the product's centre: one Leader per session, `spawn`/`delegate`/`send`/`wait`, per-member models (D-69), workspace policies shared/isolated/git worktree (D-46/D-76) |
| Headless / CI | `exec` (resume/fork/review), `review`, `cloud` | (interactive CLI) | (gateway + CLI) | `teamagents exec` with `--json`, `--stream-json` (the session's events as NDJSON while it waits, then the report; D-249), `--timeout` and `--check` (client-side acceptance), and documented exit codes (D-49, restoring the v1 D-32 contract that lives in Git history); one JSON report at the end |
| Config | `~/.codex/config.toml`, `-c key=value` overrides | provider keys/`/login`, plus `~/.pi/agent/mcp.json` and a trusted `.pi/mcp.json` | `hermes model` picker, per-platform integration config | `~/.config/teamagents/config.toml`: models, tools, skills, hooks, checks, limits and permissions (including the sandbox backend, D-369) — plus `instruction_files` and `[retention]`, which load and are reported as declared-but-not-applied (D-102/D-75); a repository-local project file is read under one opt-in (`[permissions] trust_project = true`, D-244), from the session's own directory only. No `-c` overrides |
| TUI | interactive CLI | pi-tui (differential rendering) | full TUI: multiline editing, slash autocomplete, history, **interrupt-and-redirect**, streaming tool output | ratatui TUI: conversation, panels (instances/tasks/topology), approvals box, composer history and word editing (D-77), **slash commands** (`/help` et al., D-370), a live streaming preview (D-366) and **interrupt of a running turn** (`i`, D-363); no multiline-paste editor beyond the composer and no slash *autocomplete* |
| Durability | local sessions, app-server daemon | `pi-durable` (durable conversation/task/document runtime) | serverless persistence for hibernating environments | SQLite per session (WAL + `synchronous=FULL`), one coordinator per state root, receipts consumed rather than replayed — verified live by killing the daemon mid-tool (`review/dogfood/crash.py`, A08/A11) |
| Automations | — | automation and workflows live in a separate project (`earendil-works/pi-chat`, linked from the README); the agent itself has no scheduled triggers | built-in cron scheduler with platform delivery | a config-declared scheduler the daemon runs on its own (`<state root>/automations.json`, `teamagents automations list`/`add`/`pause`/`resume`/`remove`, D-367); a due automation opens one goal and does not overlap itself, modelled by `V2Schedule` |
| Observability | `doctor`, `debug` (model catalog, prompt input, app-server) | telemetry package (vendor-neutral contracts) | session search, trajectory export for research | `doctor` (config/credentials/state/skills/isolation/tools rows, now naming the selected sandbox backend), `daemon.log`, per-session artifacts, events + receipts in SQLite, `sessions search` (D-368); evaluation evidence under `review/` |

## 2. What this comparison suggested, and where it stands now

1. **Sessions beyond one-per-state-root** (Codex: `resume`/`fork`/`archive`; Hermes: continuity across
   surfaces). **Done** in the registry sense since D-364/D-365/D-368 — named sessions under one base, with
   list/new/archive/delete/rename/restore/fork/search and `--session`. What remains is the *shared
   app-server* shape Codex has: here one state root is still owned by one daemon, and the TUI has no picker;
   it would be a second product surface rather than a protocol gap.
2. **Interrupt-and-redirect** (Hermes documents it; Codex has queueing). **Done** since D-363: a user-only
   `interrupt_instance` command performs the existing `V2Control.CancelRequest`, the supervisor aborts the
   provider stream, and the input queued behind the turn takes over (`instances interrupt --id`, TUI `i`).
3. **Streaming output for headless runs** (Codex `exec --json` streams events; Hermes streams tool output in
   the TUI) — **done since D-249**: `teamagents exec --stream-json` writes the session's committed events as
   they are observed (one `{"type":"event","event":{…}}` line each, in log order, flushed per line) and then
   the report as `{"type":"report","report":{…}}`; the TUI shows a transient live preview (D-366). It adds no
   protocol surface — it is a client of the `events(since)` read the daemon already serves — and the exit-code
   contract is unchanged (`review/dogfood/stream_json.py` measures it without a credential).
4. **Sandbox backends** (Pi: micro-VM/Docker/OpenShell; Hermes: seven backends). **Partly done since D-369**:
   the shell tool (and everything dispatched through its one spec builder, plus MCP `workspace` execution) now
   runs under bubblewrap **or** Docker, chosen in the user config, failing closed. Micro-VM/remote/SSH backends
   are still a new execution boundary with its own verification, and are not implemented.
5. **Automations** (Pi: a separate project, `pi-chat`; Hermes: built-in cron + delivery). **Done since D-367**:
   `<state root>/automations.json` declares periodic prompts, the daemon's scheduler opens one goal per due
   automation and does not overlap itself, and the schedule is modelled by `V2Schedule`.
6. **Memory/learning loop** (Hermes: self-created skills, session search, user modeling). **Still open, and
   deliberately so**: this product has no cross-session memory beyond the session database and the skills
   registry. That is an information-flow question here (`audience` vs `push`, §5.1), not a missing feature;
   the read-only half — `sessions search` across sessions — landed with D-368. **Needs the user's word** for
   anything stronger (injecting other sessions into a model's context).
7. **MCP management surface** (Codex: `mcp add/remove/login/list`; Pi: `mcp add/list` plus an in-session
   `/mcp`, OAuth, resources). Here MCP servers are declared in the config file and `doctor` reports them
   (D-74/D-78); a CLI verb would be convenience, not capability. **Low value; the user's call.**
8. **Config overrides** (Codex: `-c key=value`). This product's flags cover the session-shaping keys
   (`--cwd`, `--state-root`, `--model`, `--full-auto`); everything else is the config file. **Low value.**
9. **Project config**: done since D-244 — the product reads repository-local configuration
   (`<cwd>/.teamagents/config.toml`) under the user's `[permissions] trust_project` opt-in, which is what Codex
   and Pi do. What remains is what `docs/ACCEPTANCE.md` records: no walk up to a parent directory, and one
   opt-in for every repository rather than a per-project trust list.

## 3. What this product already does that the comparators do not (or state they do not)

- **A permission model with approvals bound to the operation and its argument hash** (D-67). Pi states it has
  none built in; Hermes' README does not describe one.
- **Verified durability**: the site's exactly-once behaviour across a daemon crash is a test-and-probe-backed
  claim here (`review/dogfood/crash.py`), not a marketing line.
- **Machine-checked acceptance**: a goal cannot be reported done while a user-defined `[[checks]]` command
  fails (D-50/A16), and a runtime block is not a success (D-71).
- **Teams as the default**: a persistent Leader with delegated tasks, per-member models and workspace
  policies, with the delegation contract (waits, task settlement) in the protocol rather than in prompts.
- **Formal verification of the protocol surfaces** (`verification/`, `make verify-model-all`,
  `make verify-model-counterexamples`): named sessions (`V2Sessions`), the scheduler (`V2Schedule`) and the
  sandbox-backend choice (`V2Isolation`) each carry a model plus refuted negative controls, which none of the
  three READMEs claims.

## 4. Honest limits of this snapshot

- Pi and Hermes rows come from the sources named above, fetched on the date above; both projects evolve.
  Pi's deeper site (pi.dev/docs) is a JavaScript application and was not read, so the Pi row rests on the
  repository itself; for Hermes one docs page (features/tools) was read in addition to the README.
- **A Pi cell was wrong and is corrected here** (2026-09-30): the Extensions cell had said Pi has **no MCP**
  (a claim the 2026-09-26 snapshot drew from the upstream tree and docs index, where it was true then). The
  upstream now holds 65 `mcp` paths and an MCP docs page, so the cell says Pi has MCP and the MCP-management
  row is no longer a Codex-only difference. The negative *worktree* claim still re-derives (no `worktree`
  path). `review/comparison_sources.py` was updated in the same change: it now requires the `mcp` paths and
  the MCP docs entry to be present, so a future removal is the finding instead.
- **Three earlier Pi cells were wrong and were corrected on 2026-09-26**: the row had claimed MCP support,
  git-worktree isolation and a scheduled-automation document; the first of those is true again as of
  2026-09-30, the second is still false, and the cited `docs/loops.md` 404s and appears nowhere in the tree.
- **Nine TeamAgents cells were stale and are refreshed here** (2026-09-30, re-read against the tree):
  *Session/resume* (sessions registry, D-364/365/368), *Permissions* and *Sandbox backends* (the Docker
  backend, D-369), *Headless/CI* (`--stream-json`, D-249), *TUI* (slash commands D-370, live preview D-366,
  interrupt D-363), *Automations* (the daemon scheduler, D-367) and *Observability* (the isolation row now
  names the backend, and `sessions search` exists). The §2 list is re-labelled from "needs the user's word" to
  what each item's status actually is.
- Codex rows come from the installed binary's help output (`codex-cli 0.156.1`), not from its documentation
  site, and were **not** re-derived on 2026-09-30 (no `codex` binary on the refreshing machine).
- No comparator was benchmarked: this is a surface comparison, not a capability or quality comparison. Surface
  parity is not parity of outcome; a real-model evaluation of task success is a separate exercise
  (`review/eval/r2-p6/`).

Re-check the two upstream rows (network only; nothing here is part of `make check`):

```bash
curl -sS "https://api.github.com/repos/earendil-works/pi/git/trees/main?recursive=1"   # 2,376 paths, truncated:false
curl -sS "https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/docs.json"  # the docs index
curl -sS "https://raw.githubusercontent.com/earendil-works/pi/main/README.md"          # automation -> pi-chat
curl -sS "https://api.github.com/repos/earendil-works/pi/contents/docs"                # 404: pi has no docs/ directory
curl -sSL "https://hermes-agent.nousresearch.com/docs/user-guide/features/tools"       # Hermes MCP toolsets
```
