# Implementation decisions (current)

This file records the direction, boundaries and deviations that are **currently binding**: any implementation
that departs from the confirmed design is discussed with the user first. The decision log of earlier
implementations stays reachable through Git history (`git log -- docs/archive`).

## Earlier rules that still apply

| Topic | Current rule | Origin |
|---|---|---|
| Skills | user-level registration root `~/.agents/skills`; read on demand with `skill search/read`, and skill instructions never widen execution permissions | D-23, D-34 |
| Usage visibility | turn and goal usage/budget are visible in the TUI and in events | D-24 |
| MCP | both transports, stdio and streamable HTTP; calls go through the shared permission, approval, budget, cancellation and receipt entry points | D-25 |
| Context compaction | triggered by real window usage; the summary keeps the original request and acceptance criteria, and the full text stays retrievable | D-28 |
| Task priority | complex coding and long-horizon stability come first | D-35 |
| Native context | real-model evaluation always uses the model's native window and records the value and its source | D-36 |
| Install and first config | download-and-run install; `init` writes a minimal config and never overwrites an existing file | D-37; `docs/INSTALL.md` records the differences of earlier releases (≤ v0.1.2) |
| Custom providers | any compatible service is configured through `[models.*]` in `config.toml` (`protocol`/`base_url`/`model`/`api_key_env`) | D-40 (the earlier TUI's `/model` wizard went away with the old interface) |
| full_auto | user-only host shell (D-41); the default `approved_scope` runs under bubblewrap | D-41 |

## D-46 Workspace policies wired into spawn (2026-09-25)

D-45 found that the shared/isolated/git-worktree policies in `engine/src/workspace.rs` ([design](DESIGN.md)
§12.3, Q14) were only called by their own unit tests. After the user confirmed "wire it up":

- **Model-visible entry**: the `spawn` tool gains an optional `workspace` argument — `shared` (default: the
  project directory), `isolated` (a private directory at `<state root>/instances/<id>/work`) or
  `git_worktree` (the instance's own branch and worktree). An unknown value fails that tool call (a
  `collaboration`-class receipt) and **never** kills the driver.
- **Resolution and record**: `driver::prepare_spawn_workspace` resolves the policy before the instance
  starts, creates the directory or worktree and writes the result to `<instances_dir>/<id>/workspace.json`
  (atomic replace). `workspace_ref` points at the resolved directory and the tool receipt carries
  `path/policy/note`, so the model can see whether it got a shared or isolated workspace and why a fallback
  happened.
- **Fallback**: asking for a worktree in a project that is not a Git repository or has uncommitted changes
  runs in shared mode and says why in `note` — uncommitted input is never ignored silently.
- **Retirement**: when an instance reaches `TERMINATED` the supervisor retires the workspace from its record.
  A shared record only drops the record; an isolated directory that holds anything besides our own
  `INPUTS.md`, or a worktree with uncommitted or unmerged work, is **refused and reported** (the site is
  kept). Everything else is removed together with the record, and a failed retirement never affects
  termination itself.
- **No garbage on failure**: when the control plane refuses a spawn, the prepared directory is kept (nothing
  is deleted on a guess) and a retry with the same instance id reuses it.
- Evidence: `engine/src/workspace.rs` unit tests (policy, fallback, record, retirement, including "uncommitted
  work is not deleted"), `engine/tests/v2_driver.rs::spawn_resolves_the_requested_workspace_policy` (isolated
  and worktree rows plus receipts; an unknown policy is refused without creating a directory) and
  `engine/tests/v2_supervisor.rs::terminating_an_instance_retires_its_workspace` (the directory and its record
  are retired, the shared project stays). Docs: `docs/USER-GUIDE.md` §3 and the README feature list.
- Side cleanup: `AgentSpec`/`RuntimeKind` in `core/src/models.rs` were only used by the old signature and
  went away with the change to `prepare(id, policy, project_cwd, member_dir)`.

## D-45 Cleaning earlier-implementation leftovers and restoring `[hooks]` (2026-09-25)

The user confirmed, item by item: (1) rewrite history; (2) remove the earlier implementation's code;
(3) drop doctor's Codex probe; (4) delete the two `#[ignore]`d old entry-point tests; (5) restore `[hooks]`
with the existing wire protocol; (6) keep `review/tmp/` as the probe area; (7) push.

- **hooks (5)**: `engine/src/hooks.rs` keeps the existing wire protocol — for `notify`, argv[1] is the event
  name and the event JSON arrives on stdin; it is asynchronous, bounded at 10 seconds and only logs failures
  to stderr. `pre_tool` runs synchronously before every native tool call: exit 0 allows, exit 2 denies (the
  first stderr line becomes the reason handed to the model) and any other exit code, spawn failure or timeout
  allows the call while logging to stderr. The event set is `tool_call` (with
  `tool`/`arguments`/`ok`/`error`), `team_action`, `run_completed`, `run_failed`, `run_cancelled` and
  `run_paused` (the edge into PAUSED; the value seen at boot does not count). A replay after crash recovery
  is not asked again (the decision was made at first dispatch), and required checks are the user's own
  acceptance commands rather than model tool calls, so they skip `pre_tool`. Evidence: the `hooks.rs` unit
  tests plus `a_pre_tool_hook_vetoes_a_tool_call_and_the_turn_continues` and
  `notify_hooks_receive_tool_call_and_run_completed` in `engine/tests/v2_driver.rs`; doctor still checks that
  hook programs are executable.
- **Old control plane removed (2)**: deleted `core/src/{control,storage,views,server,references}.rs`, the
  `teamagents-core` stdio binary, the six test files that existed for it and the `core/src/models.rs` types
  only those used; `BUILTIN_TOOL_BINDINGS` moved to its single product use site, `engine/src/bound.rs`. Core
  dropped from 243 to 91 test cases, keeping every case the product path and the acceptance matrix cite
  (`core/src/v2/control.rs` unit tests, `v2_invariants`, `kernel_properties`).
- **Doctor's Codex probe (3)**: the current implementation has no Codex member type, so the
  `codex app-server` and `codex protocol schema` checks and their helpers were removed. The
  "no longer supported" errors for `--resume/--team/--plain` stay, because a clear message beats silently
  ignoring the argument.
- **Two old entry-point tests (4)**: they could only fail if enabled (they assert the removed entry points
  return `ok:`), so they went away with the code.
- **History rewrite (1)**: `verification/tla/states/` (TLC state files, roughly 23 GB uncompressed) appeared in
  two unpushed commits only and was removed with
  `git filter-repo --path verification/tla/states --invert-paths`. Verification: `HEAD^{tree}` and
  `git ls-files` are identical before and after (content unchanged), the path has no objects left in history,
  and the pack dropped from 6.18 GiB to about 27 MiB. The pre-rewrite `.git` backup was kept next to the
  repository and deleted once the result was confirmed. The commit id cited in `verification/REPORT.md`
  (`bc536bb5`) was updated to the rewritten `d37e1b4`.
- **Workspace policies**: untouched here; the user then chose "wire it up", see D-46.

## D-44 Two fixes found by formal verification (2026-09-24)

Landed after the user confirmed "fix everything". Both findings started as counterexamples from the TLA+/TLC
specs and were confirmed with code probes; each has a regression test. The fix ledger is in
[review/fix-notes-verification-2026-09-24.md](../review/fix-notes-verification-2026-09-24.md).

- **V-W1: a wait's tool_call was not answered.** Only the drain path answered it; the "satisfied at
  registration" and "superseded/closed epoch" paths moved the wait to SATISFIED/CANCELLED without appending
  a tool response, so a strict wire endpoint rejects the next request. Fix: `answer_closed_waits` extends the
  answer to both paths (same reason text as the drain, deduplication key stays the wait id). Spec side:
  `ResolvedWaitIsAnswered`; regression `wait_call_answered_outside_the_drain_path`.
- **V-G1: a settled goal still accepted new work.** Delegation, opening operations and continued billing all
  ignored the goal status, so new turns kept billing a settled goal. Fix: `budget_goal` accepts only ACTIVE
  goals (both the instance pointer and the oldest open task path), `complete_goal`/`block_goal` detach the
  instance pointer on settlement (`detach_goal`, with `detached` in the response and events), and
  `delegate_task` requires an ACTIVE goal and tells the caller to create one first. Spec side:
  `NoStaleActiveGoal`, `RegisteredWorkNeedsAnActiveGoal`, `RequestsResolveToActiveGoals`; regression
  `a_settled_goal_takes_no_new_work`.
- **Deliberate semantic boundary**: the linearization point for "new work" is the request, not the operation.
  A request admitted while the goal was ACTIVE may still open operations and bill that goal after settlement —
  that is honest accounting, not new work. `complete_goal` checks open operations but not tasks, so a goal can
  settle while its own tasks are still open and those tasks' later requests have no billing goal; tightening
  that would need a committed "completion refused" result for the driver and is out of scope here.
- **V-P1: stale execution pointer after termination.** The spec-to-code correspondence test
  (`core/tests/v2_invariants.rs`) found, during a random walk, an instance whose phase stayed
  `MODEL_PENDING` with `active_request_id` pointing at a cancelled request. Fix: the termination branch
  normalizes the execution pointer exactly like `reset_instance`/`fail_request`. Regression
  `terminating_an_instance_normalizes_its_execution_pointer`.
- **V-P2: a compression request could be imported as a turn.** The same correspondence test reached
  `import_response` on a compression request and it succeeded. Fix: the control plane refuses imports whose
  `kind != 'turn'` (compression is submitted by `compress_context`); regression
  `import_response_refuses_a_compression_request`.
- Evidence: `make verify-model-all` (control plane, artifacts, waits, tasks and compression all exhaustively
  green) and `make check` (including `core/tests/v2_invariants.rs`: all command sequences up to length 2, 60
  fixed-seed walks, coverage assertions and a negative control for checker sensitivity). The property ↔ code
  ↔ acceptance-item mapping is in [verification/README.md](../verification/README.md).

## D-43 Provider edges aligned with the pi coding agent (2026-09-24)

The user asked for multi-provider support to follow the pi coding agent directly (the `pi-ai` package in the
`earendil-works/pi` repository). The gaps from the earlier comparison were landed by current impact:

- **Landed**: tool-call ids normalized across protocols (the same id maps consistently within one request) and
  `max_tokens` clamped to the remaining context (4096 safety margin) — both had already landed earlier. This
  round added provider-independent retry-text classification (a 429 carrying quota/billing-exhausted wording
  becomes Permanent before the status-code table) and effort normalization at the config edge (deepseek
  xhigh→max keeps the existing user decision; anthropic xhigh/max→high follows pi's `clampReasoning`; anything
  else passes through, since a catalog entry is the user's declaration of what the model supports).
- **Not landed yet**: image downgrading (the runtime has no image flow; a `ponytail:` comment records the pi
  style upgrade path — declare input modalities in the catalog plus a placeholder at the edge), a cost rate
  catalog (A18 bills tokens, not dollars yet) and more protocol adapters such as google/vertex/bedrock (add
  them when one is needed; the adapter pattern is in place).

## D-42 System direction and scope confirmed (2026-09-23)

After 19 requirement clarifications and a re-examination of the Python kernel and LangGraph, the user
explicitly chose: "a small Rust kernel + a persistent Rust instance runtime + SQLite + separate tool process
management + a Rust TUI", and asked for every rationale and alternative to be re-examined. This record
confirms the system direction and scope.

- The product stays in Rust. The small kernel owns concise model interaction and execution decisions, and the
  persistent instance runtime owns long-horizon tasks, permissions, scheduling, recovery and tool execution;
  Python/LangGraph are not a premise.
- A team of one is legal; instances isolate context, messages and tool access by default. The Leader manages
  by default and can delegate a limited subset; authorized instances may talk directly and the connection
  graph may be arbitrary, replacing D-33's member-to-member restriction. The shared project directory is
  granted by default, with isolated directories or worktrees on demand; `full_auto` is still only logical
  isolation.
- The first usable version includes the Rust TUI, basic file/shell/web tools, MCP, Skills, multiple providers
  and mixed models inside one team. No external Codex backend is needed; short-lived helpers use the same
  instance mechanism, and D-39's separate helper loop is no longer an architectural requirement.
- Background work survives CLI/TUI exits; the user can reconnect, read any instance's history, talk directly
  to an instance and pause or cancel it. Those user permissions are not automatically granted to the Leader or
  other agents. Instances are reused within a session, can be terminated or reset, and are isolated across
  sessions by default.
- Work continues by default with an optional goal-level budget, and permanent failures or repeated failures
  are handled in a bounded way. A restart resumes authorized work; when an external outcome is unknown it is
  verified first, and if it still cannot be confirmed the affected tasks park and notify instead of being
  replayed blindly.
- Required checks must pass and every other completion claim carries evidence and unverified items;
  independent review happens on demand and never forces extra instances. Acceptance weighs success rate and
  long-horizon reliability at the same model and budget: single-instance behaviour must not regress and
  on-demand collaboration must show a reproducible gain. Costs are estimated on a small scale before the
  formal evaluation budget is set; this item contains no real-model calls.
- DeepSeek Flash is the user-confirmed DeepSeek V4.1 Flash and the main acceptance baseline with its native
  1,000,000 context. D-36's native-window rule and D-41's `full_auto`/`approved_scope`, process-group
  cancellation and credential-environment semantics stay in force.
- No compatibility with old configs or sessions is required; old sessions, run state, caches and old configs
  may be cleaned up, while credentials, raw evaluation records, review evidence, other applications' data and
  Git history are kept. Cleanup runs from an ownership inventory during a switch; this item deleted no data.

The full requirement mapping and acceptance are in the [design baseline](DESIGN.md). The engineering arguments
about the single-database atomic boundary, the runner handshake, the I/O candidates and concurrency defaults
are in the 45-item design review (reachable through Git history: `git log -- review/archive`). Those arguments
are not measured performance results, and they do not mean the user approved each pending library, parameter
or statistical precision. Requirements that conflict with this item's confirmed scope are superseded by it;
untouched behaviour contracts remain in force.
