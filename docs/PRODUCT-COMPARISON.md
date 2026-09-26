# How this product compares with Codex CLI, Pi and Hermes

Snapshot taken **2026-09-26** for the product direction the user set ("reference Codex CLI, pi and hermes").
It exists to make the *decisions* explicit, not to declare parity: every row says where its fact comes from,
and the last section lists what follows for this repository.

**Sources, and how strong they are**

| Reference | Source | Strength |
|---|---|---|
| Codex CLI | the installed binary here, `codex-cli 0.156.1` (`codex --help`, `codex exec --help`, `codex mcp --help`, `codex sandbox --help`, `codex debug --help`, `codex app-server --help`) | re-checked locally on 2026-09-26: every claimed verb appears in that help output |
| Pi | the upstream README, its docs index (`packages/coding-agent/docs/docs.json`) and its file tree (GitHub API, 2,162 paths, `truncated:false`) | re-derived 2026-09-26: the README alone cannot support a **negative** claim, so the row was checked against the tree and the docs index |
| Hermes | the upstream README (`NousResearch/hermes-agent`) plus the features/tools page of its docs site | re-checked 2026-09-26: seven backends, cron, TUI and MCP toolsets all confirmed there |
| TeamAgents | this repository: `docs/USER-GUIDE.md`, `docs/DECISIONS.md`, `docs/ACCEPTANCE.md`, `review/dogfood/*` | the evidence in this tree |

Pi and Hermes are documented here from their READMEs — they were **not** run, so their rows describe what
their projects claim. Codex was run (its help output is the evidence above).

## 1. By dimension

| Dimension | Codex CLI | Pi | Hermes | TeamAgents (2026-09-27) |
|---|---|---|---|---|
| Session / resume | `resume` (picker / `--last`), `fork`, `archive`, `delete`, `migrate-rollouts`, and `agents` to browse sessions on a *shared local app-server daemon* | resumable sessions, session history | conversation continuity across platforms, platform gateway | one session per state root, owned by one daemon (A33); the TUI and `exec` attach to it. No picker, no fork, no archive: a second instruction after a settled goal is a known gap |
| Permissions | config + `-c` overrides, `sandbox` subcommand | **none built in** ("runs with the permissions of the user and process that launched it"; containerize it for boundaries) | platform/sandbox backends | `approved_scope` (bubblewrap, approvals bound to the operation and its argument hash, D-67) or `full_auto` (host shell, D-41); capability grants with a user-facing surface (`teamagents authority`, D-61); the mode now also comes from the user config (D-75) |
| Sandbox backends | Linux sandbox + config | micro-VM / Docker / OpenShell patterns | seven terminal backends (local, Docker, SSH, Singularity, Modal, Daytona, Vercel Sandbox) | bubblewrap only, for every command the product *executes*: the shell tool — which also runs the `[[checks]]`, the client's `--check` and the job runner's commands, all from one spec builder (`tools::shell_command_spec` → `tools::bwrap_argv`) — and MCP servers with `mcp_execution = "workspace"` (the default; `engine/src/mcp.rs` builds from the same argv). `mcp_execution = "host"` and `full_auto` are the explicit opt-outs. A missing or unusable bubblewrap is a classified failure, never a silent host fallback (A14) |
| Tools | files, shell, apply (`codex apply`), review | files, shell, `!` commands, extensions | files, shell, scripts calling tools over RPC | files, shell (isolated or host), web (fetch/search — offered only for the kinds the config *declares*, D-168, and search needs a credential, D-167), MCP, skills; every tool call is an operation with a durable receipt (A08/A30) |
| Extensions | `mcp add/remove/login`, `plugin`, `features` | skills, prompt templates and extensions (upstream `packages/coding-agent/docs/skills.md` and `extensions.md`); **no MCP** — no MCP page in the upstream docs index and no path in its tree contains "mcp" | MCP, skills (agentskills.io compatible), memory providers | MCP over stdio + streamable HTTP, declared in `[tools.*]` (D-74); skills from `~/.agents/skills` and configured roots (A26, D-34/D-66); user hooks `pre_tool`/`notify` (D-45, live evidence D-92) |
| Teams / subagents | (single agent per session; `fork` for branches) | one subagent extension example (one process per subagent, isolated context windows; ≤8 tasks, 4 concurrent); **no worktree isolation** — no path in the upstream tree contains "worktree" | subagents for parallel workstreams | the product's centre: one Leader per session, `spawn`/`delegate`/`send`/`wait`, per-member models (D-69), workspace policies shared/isolated/git worktree (D-46/D-76) |
| Headless / CI | `exec` (resume/fork/review), `review`, `cloud` | (interactive CLI) | (gateway + CLI) | `teamagents exec` with `--json`, `--timeout`, `--check` (client-side acceptance) and documented exit codes (D-49, restoring the v1 D-32 contract that lives in Git history); one JSON report at the end — **no streaming event output** |
| Config | `~/.codex/config.toml`, `-c key=value` overrides | provider keys/`/login` | `hermes model` picker, per-platform integration config | `~/.config/teamagents/config.toml`: models, tools, skills, hooks, checks, limits and permissions — plus `instruction_files` and `[retention]`, which load and are reported as declared-but-not-applied (D-102/D-75); a project file is *not* read yet (known gap) |
| TUI | interactive CLI | pi-tui (differential rendering) | full TUI: multiline editing, slash autocomplete, history, **interrupt-and-redirect**, streaming tool output | ratatui TUI: conversation, panels (instances/tasks/topology), approvals box, composer history and word editing (D-77); no slash commands; no interrupt of a running turn (the open D-63 question) |
| Durability | local sessions, app-server daemon | `pi-durable` (durable conversation/task/document runtime) | serverless persistence for hibernating environments | SQLite per session (WAL + `synchronous=FULL`), one coordinator per state root, receipts consumed rather than replayed — verified live by killing the daemon mid-tool (`review/dogfood/crash.py`, A08/A11) |
| Automations | — | automation and workflows live in a separate project (`earendil-works/pi-chat`, linked from the README); the agent itself has no scheduled triggers | built-in cron scheduler with platform delivery | none (a goal runs when the user asks) |
| Observability | `doctor`, `debug` (model catalog, prompt input, app-server) | telemetry package (vendor-neutral contracts) | session search, trajectory export for research | `doctor` (config/credentials/state/skills/isolation/tools rows), `daemon.log`, per-session artifacts, events + receipts in SQLite; evaluation evidence under `review/` |

## 2. What this comparison suggests, in decision order

1. **Sessions beyond one-per-state-root** (Codex: `resume`/`fork`/`archive`; Hermes: continuity across
   surfaces). Today the state root *is* the session (A33), which keeps recovery and the coordinator lock
   simple. A picker/fork/archive surface would be new protocol and new product surface, and it interacts with
   the settled-goal gap already recorded in `docs/ACCEPTANCE.md` (a second instruction cannot build a team).
   **Needs the user's word.**
2. **Interrupt-and-redirect** (Hermes documents it; Codex has queueing). This is exactly the open question of
   D-63 ("should the runtime interrupt a running turn instead of holding the input to the boundary?"). The
   substrate exists and is parked with a note (`driver::cancel_turn`, `TurnControl::wait_idle`). **Needs the
   user's word.**
3. **Streaming output for headless runs** (Codex `exec --json` streams events; Hermes streams tool output in
   the TUI). `teamagents exec` prints one report at the end; the TUI already consumes streamed *previews* that
   are explicitly not authoritative (§9). A `--stream-json`-style mode is additive surface, no design change.
   **Needs the user's word** only because it adds protocol-visible output.
4. **Sandbox backends** (Pi: micro-VM/Docker/OpenShell; Hermes: seven backends). This product ships
   bubblewrap for the shell tool and says so (A14). Anything else (container/micro-VM/remote sandbox) is a new
   execution boundary with its own verification. **Needs the user's word.**
5. **Automations** (Pi: a separate project, `pi-chat`; Hermes: built-in cron + delivery). Nothing here runs unattended on a schedule;
   a scheduled trigger would need its own admission rules (who may start a turn, with which budget).
   **Needs the user's word.**
6. **Memory/learning loop** (Hermes: self-created skills, session search, user modeling). This product has no
   cross-session memory beyond the session database and the skills registry. That is a deliberate
   information-flow question here (`audience` vs `push`, §5.1), not a missing feature. **Needs the user's
   word.**
7. **MCP management surface** (Codex: `mcp add/remove/login/list`). Here MCP servers are declared in the
   config file and `doctor` reports them (D-74/D-78); a CLI verb would be convenience, not capability.
   **Low value; the user's call.**
8. **Config overrides** (Codex: `-c key=value`). This product's flags cover the session-shaping keys
   (`--cwd`, `--state-root`, `--model`, `--full-auto`); everything else is the config file. **Low value.**
9. **Project config** is a gap of this repository's own making (the loader exists, no entry point calls it,
   `docs/ACCEPTANCE.md`). Codex and Pi both read repository-local configuration. **Needs the user's word**
   (it changes what a cloned repository can influence).

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
  `make verify-model-counterexamples`), which none of the three READMEs claims.

## 4. Honest limits of this snapshot

- Pi and Hermes rows come from the sources named above, fetched on the date above; both projects evolve.
  Pi's deeper site (pi.dev/docs) is a JavaScript application and was not read, so the Pi row rests on the
  repository itself; for Hermes one docs page (features/tools) was read in addition to the README.
- **Three Pi cells were wrong and are corrected here** (2026-09-26): the row had claimed MCP support, git-worktree
  isolation and a scheduled-automation document. The upstream tree has no `mcp` path and no `worktree` path, the
  docs index has no MCP page, and the cited `docs/loops.md` 404s and appears nowhere in the tree. The cells now
  state what the upstream tree and docs index actually contain.
- **Four TeamAgents cells were wrong or stale and are corrected here** (2026-09-27, re-read against the tree):
  *Sandbox backends* said bubblewrap was "only for shell", but `tools::shell_command_spec` → `tools::bwrap_argv`
  is the single spec builder for the shell tool *and* everything dispatched through it — the `[[checks]]`, the
  client's `--check`, the job runner's commands — and `engine/src/mcp.rs` builds MCP `workspace` execution from
  the same argv. *Tools* now says the web half is offered only for the kinds the config declares (D-168) and that
  search needs a credential (D-167). *Config* was missing the two sections that load and are reported as
  declared-but-not-applied (`instruction_files`, `[retention]`). *Extensions* cited `D-53/D-79` for the user
  hooks; the binding decision is **D-45** (live evidence: D-92), while D-53 is a docs-correction entry and D-79 is
  about the web tools. The comparator cells are unchanged — their sources are dated in the table above.
- Codex rows come from the installed binary's help output (`codex-cli 0.156.1`), not from its documentation
  site. That re-check removed one claim: the Observability row said Codex had "traces", and no `codex` subcommand
  offers a trace surface (`debug` renders the model catalog, the prompt input and app-server tooling).
- No comparator was benchmarked: this is a surface comparison, not a capability or quality comparison.

Re-check the two upstream rows (network only; nothing here is part of `make check`):

```bash
curl -sS "https://api.github.com/repos/earendil-works/pi/git/trees/main?recursive=1"   # 2,162 paths, truncated:false
curl -sS "https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/docs.json"  # the docs index
curl -sS "https://raw.githubusercontent.com/earendil-works/pi/main/README.md"          # automation -> pi-chat
curl -sS "https://api.github.com/repos/earendil-works/pi/contents/docs"                # 404: pi has no docs/ directory
curl -sSL "https://hermes-agent.nousresearch.com/docs/user-guide/features/tools"       # Hermes MCP toolsets
```
