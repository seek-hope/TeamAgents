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

## D-48 TUI shortcuts without function keys (2026-09-25)

The user pointed out that some keyboards have no function keys, so the interface no longer binds any. View
switching is now:

| Key | Effect |
|---|---|
| `Ctrl+N` | cycle the views: conversation → instances → tasks → topology → conversation (works while composing) |
| `Ctrl+A` | jump into the approvals box (only when approvals are pending) |
| `Esc` | back to the conversation from a panel (unchanged) |
| `Tab` | switch the conversation target (unchanged) |
| `Ctrl+C` / `Ctrl+D` | quit (unchanged) |

The removed bindings were `F1` (conversation), `F2` (approvals), `F3` (instances), `F4` (tasks) and `F5`
(topology). Panel-local keys (`Enter`, `p`, `r`, `t`, `c`, arrows) are unchanged, and the footer hint line now
advertises `Ctrl+N` / `Ctrl+A` instead of the function keys. Control chords never insert text into the
composer, so a `Ctrl+<letter>` press can no longer leave a stray character behind.

Evidence: `tui/tests/v2app_tests.rs::view_switching_cycles_with_ctrl_n_and_esc_returns` walks the cycle,
asserts that pressing `F3` leaves the view unchanged and that the hint mentions `Ctrl+N`;
`instances_panel_pauses_resumes_and_switches_the_conversation`, `tasks_panel_cancels_only_live_tasks`,
`termination_requires_an_explicit_confirmation` and `the_palette_drives_panels_selection_and_status` reach
their panels through the cycle. `make pty` drives the real terminal with the `Ctrl+N` bytes, and `make check`
is green.

## D-47 TUI colour scheme: the v1 palette (2026-09-25)

The user preferred the v1 TUI's colours over the ones the current conversation UI used. The palette is restored
as `tui/src/theme.rs` (the Codex palette: `BG` 0x0d0d0d, `PANEL_BG` 0x181818, white foreground, `GREY` 0x5d5d5d,
`ACCENT` 0x3b82f6, green success, `NOTICE` 0xafafaf, red error, yellow warning plus `SELECT_BG`/`ZEBRA_BG`/
`HOVER_BG`) and applied to the current UI:

- the whole screen sits on `BG`;
- every bordered panel uses an `ACCENT` border, a `PANEL_BG` surface and an accent title;
- the selected list row is the accent surface with panel-dark text (v1's selection look);
- the status line sits on `PANEL_BG`, and turns dark-on-red while disconnected;
- chat labels follow the v1 convention: grey bold labels, white body text, accent for the assistant, notice
  grey for machine-generated text and red for errors; the footer is grey.

The v1 TUI itself (its layout, tabs, forms and slash commands) is **not** restored: it drove the retired
backend. Only the colour scheme was ported, which is what was asked; further v1 interface elements can be
ported one by one on request (the source stays in Git history, `git log -- tui/src/ui.rs`).

Evidence: `tui/tests/v2app_tests.rs::the_palette_drives_panels_selection_and_status` asserts the screen
background, the accent panel border, the accent-surface selection and the panel-surface status line on a
TestBackend frame; `make check` and `make pty` are green.

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
## D-77 The composer's history and word editing are wired (2026-09-26)

The dead-code sweep that produced D-74/D-75/D-76 looked at every `pub fn` with no caller, and
`tui/src/text.rs` stood out: its module header says "↑↓ recall history at the first/last row", the composer
keeps a 500-deep history with `record_submission`/`recall` (draft preserved, adjacent duplicates skipped) and
**has a unit test for exactly that** — and `v2app` called none of it. Nothing ever recorded a submission, so
the history was always empty; `↑`/`↓` scrolled the conversation instead (PageUp/PageDown and the mouse wheel
have always done that too); `Ctrl+←`/`Ctrl+→`/`Ctrl+W` (`move_word_left`/`move_word_right`/`delete_word`) and
`clear_composer` were unreachable. A first-class coding CLI whose composer cannot recall the prompt you just
sent is missing a basic affordance, and the module *documented* it.

**What is wired now** (nothing new was invented — the behaviour is the one `text.rs` already defined):

- a submitted prompt is recorded (`record_submission`) before the turn is sent;
- `↑`/`↓` walk a multi-line draft row by row and recall history at its first/last row, with the draft restored
  when you walk past the newest entry (the rule in the module header, now also in the footer hint: `↑ history`);
- `Ctrl+W` deletes the word before the caret, `Ctrl+←`/`Ctrl+→` jump by word;
- scrolling stays on `PageUp`/`PageDown` and the wheel (this is the one user-visible change: the arrows are the
  composer's now, which is also what the sibling CLIs do);
- `clear_composer` had no caller and no plausible binding, so it is gone rather than left as a dead affordance.

Evidence: `tui::the_arrows_recall_what_was_submitted` (two sends, then ↑↑↑ clamps at the oldest entry, ↓↓
restores the draft), `tui::the_arrows_walk_a_multi_line_draft_before_recalling` (the caret walks
`alpha\nbeta` first and the draft comes back with its newline), `tui::word_wise_editing_is_wired`
(`Ctrl+W` twice leaves `"alpha "`, `Ctrl+←`/`→` land on the word boundaries); and — the part a unit test
cannot reach — `make pty` now sends the terminal's own `↑`/`↓` escape sequences: the recalled prompt is
submitted a second time as a real `submit_input` frame, and `↓` back to the empty draft sends nothing (that
check fails with the recording removed). README's TUI key list and the footer hint describe the new keys.

Ceiling: the history is per composer (one session, in memory — it does not survive a TUI restart), and the
recall is textual: there is no search, no cross-instance history and no persistence. Whether the history
should live in the session state (so a restarted TUI still recalls) is a design question for the user.

## D-76 The workspace lifecycle, observed end to end (2026-09-25)

D-46 wired the §12.3 policies and promised, in the docs, that a terminated instance retires its workspace and
that uncommitted or unmerged work is never deleted — with unit tests and one driver test as evidence. Nothing
had observed the *whole* lifecycle with a real model, which is where the two interesting questions live: does
a member's tools really work inside its own worktree (the D-57 class: a dogfooding run once spent sixty turns
in the wrong tree), and what does the user actually do with a worktree's branch?

**The harness** `review/dogfood/workspace.py` builds a real git repository, has the model spawn a
`git_worktree` worker and delegate a file write to it, and then walks the lifecycle. Measured 2026-09-25:
deepseek 16.4 s / kimi 41.8 s, both `goal_status: SUCCEEDED`, the member's record showing
`policy: git_worktree`, and:

- the file the member wrote is **in its worktree and not in the shared project** (`git worktree list` names
  the checkout), so the isolation claim holds with a model in the loop;
- `teamagents instances terminate --id <id> --yes` succeeds while the work is uncommitted, the checkout is
  **kept**, and the daemon reports
  `workspace of <id> kept: the worktree has uncommitted, ignored or conflicting files; keep the results before cleaning up`;
- after the probe commits and merges the branch, the **running** session retires the checkout on its own next
  pass, record included (the retirement loop visits every TERMINATED instance, so a merge performed later is
  still picked up — a behaviour nothing tested before).

**One defect came out of it**: the retirement runs on *every* discovery pass, so that refusal was printed at
the poll rate (the harness produced five identical `daemon.log` lines in 1.5 s; a user who leaves an unmerged
worktree overnight would write ~50 MB). The supervisor now remembers the last reported refusal per instance
and reports the same reason once — a *different* reason, or a refusal after a successful retirement, is news
again (`supervisor::tests::a_workspace_refusal_is_reported_once_per_reason`). Re-measured: exactly one line
per reason, in the same scenarios, on both providers.

**Ceiling, and a gap this exposes**: the merge is the *user's* (or a Leader's, through `shell@workspace`),
because no surface merges a member branch: `workspace::merge_branch` and `workspace::member_worktrees` have no
caller anywhere in the tree (their doc comment calls the first one a "Leader-side merge helper"), and the
branch name is discoverable only from `<state root>/instances/<id>/worktree.json` or `git worktree list`.
That is recorded in ACCEPTANCE's known gaps: a `teamagents instances merge --id` verb (or a Leader-side merge
tool) is new product surface and needs the user's word before it lands.

## D-75 Config keys that did nothing now either work or say so (2026-09-25)

After D-74 the sweep continued over the config surface itself: every field of `UserConfig`, `ModelProfile` and
`ToolBinding` was checked against its read sites. Three keys had none beyond the `doctor` row and their own
parsing tests — accepted by the loader (`deny_unknown_fields` makes them *known*, so nothing complains) and
invisible to the runtime.

| Key | What it promised | What it did |
|---|---|---|
| `[permissions] mode` | "Full-auto must be user-chosen in config or CLI" (the code's own comment); `permission_mode_from_config()` exists with a dedicated error for a bad value | `cli::daemon_boot` built the mode from the flag alone, so a user who wrote `mode = "full_auto"` silently ran in `approved_scope` — the safe direction, and still a lie: their out-of-scope calls parked on approvals they had switched off. The helper had **no caller** anywhere |
| `[retention] archived_days` / `history_days` | "Delete archived sessions untouched for this many days when a session is opened" / "Drop applied deliveries and events older than this many days" (the struct's own comments; DESIGN §9 promises ordinary history is "archived or cleaned per user configuration", while live references and evaluation evidence are never evicted) | nothing: no code path archives, prunes or deletes, and `doctor` printed `[ok ] retention archived_days=30 history_days=7`, which reads as "in effect" |
| `models.*.codex_profile` | layer `$CODEX_HOME/<name>.config.toml` through `codex --profile <name> app-server`, so the Codex profile owns provider, model and credentials | nothing, and DESIGN Q12 excludes an external Codex adaptation from this release; a config carrying it ran the shipped provider while the user believed Codex owned the credentials |

**What each one gets, and why it differs:**

- **`mode` now works.** The user's config is the session default and `--full-auto` still asks for host
  execution for one boot; a project file can never set it (`permission_mode_from_config` reads the user config
  only), so D-41's "user-only" rule is intact and the surface matches the sibling CLIs the user pointed at
  (their sandbox/approval policy lives in the config file). Evidence:
  `cli::full_auto_reaches_a_started_daemon_and_is_reported_against_a_live_one` now boots three sessions —
  config `full_auto` → the greeting says `full_auto`, config `approved_scope` → `approved_scope`, flag over
  config → `full_auto`. With the pre-fix line restored the same test fails (`left: "approved_scope",
  right: "full_auto"`).
- **`retention` is reported, not implemented.** Deleting history is destructive and the design ties it to
  conditions that need their own verification (ordinary history may be cleaned; live references and
  evaluation evidence may never be evicted), so the honest step now is that `doctor` stops implying it works:
  the row is a WARN saying the numbers "are not applied: this release never archives or prunes a session, so
  nothing is deleted". Implementing it is the user's call and is recorded in ACCEPTANCE's known gaps.
- **`codex_profile` is refused.** A key that changes which provider and credentials a member uses must not be
  ignored; the loader now fails with `models.<key>.codex_profile = …: an external Codex profile is not part of
  this release; configure the member directly with provider/protocol/base_url/api_key_env`
  (`config::a_codex_profile_is_refused_instead_of_ignored`). The field stays declared — with its comment
  corrected — so a future release can implement it deliberately.

Ceiling: this closes the three keys that existed, not the class. The rule it restates (and the reason each
case is handled differently) is: **a config key this build does not serve is either made to work, refused
with a pointer, or reported as not in effect — never accepted in silence.**

## D-74 A configured MCP service is bound by declaring it (2026-09-25)

The audit of A25 ("MCP approval / cancellation / unknown outcome", D-25) asked a question the tests could not
answer: how does a *user* bind an MCP service? The protocol side is well covered (stdio and streamable HTTP
through fake servers, the approval/cancel/unknown-outcome paths), the design lists MCP as a shipped feature,
the user guide documents `[tools.web]` as the "optional tool binding (web / fetch / mcp)" section — and the
answer was: **you cannot**.

`BoundTools::load_in` loads a service when its *name* appears in the member's bindings list. That list is
built by the product — `cli::daemon_boot` passes `["files", "shell", "web", "skills"]` and nothing else
anywhere in the tree — so an entry like

```toml
[tools.probe]
kind = "mcp"
command = "/usr/bin/python3"
args = ["-u", "probe_server.py"]
```

was parsed, validated by `doctor`, loaded into the merged (trust-filtered) catalog, handed to every driver…
and never selected. The probe makes it visible: a server that records its own start writes nothing, because
it is never spawned — no error, no warning, just a capability that is not there. (The web half of the same
section has always worked, because a `web_search`/`web_fetch` entry is selected whenever `web` is bound; MCP
was the only kind that needed a name no surface could provide.)

**The fix is one rule**: a `[tools.<name>] kind = "mcp"` entry in the merged catalog *is* the user's binding
of that service, exactly as it already is for the web tools. The catalog that reaches the loader is the
user config merged with the project config under the documented trust rule (`[permissions]
trust_project_tools = true`, or the project's tools are dropped — `config::load_user_config_for`), so a
cloned repository still cannot bind anything on its own. Names in the bindings list keep working (an unknown
or unsupported one is still refused), and a service marked `required` still fails the member's start loudly —
which now means a mistyped `command` in a `required` entry stops the session at boot.

**`doctor` reports each declared service first** (the D-66 rule: a config surface a user cannot see is a
trap): a row per `[tools.*]` MCP entry saying whether its command is runnable (or its http url present), that
it runs over which transport, and whether it is required. A `required` service with an unrunnable command is
now visible before the session boots rather than only in `daemon.log`.

Evidence (all re-runnable):

- `bound::a_declared_mcp_service_is_bound_without_naming_it_in_the_bindings` — the product's own bindings
  list plus one declared service: the bound tool appears (`probe_probe_ping`, i.e. `<service>_<tool>`) and is
  the schema the model call advertises, the server really started, and a name the catalog does not define is
  still refused. With the pre-fix rule restored the same test fails (`left: [], right: ["probe_probe_ping"]`).
- `v2_supervisor::a_configured_mcp_service_reaches_the_members_surface` — the leader's *offered* tool list
  from a real session carries `probe_ping` next to the built-ins (fails pre-fix: the surface is
  `["shell","wait","send","delegate","spawn","finish","read_history"]`).
- `cli::doctor_probes_isolation_and_config_errors` — the new rows: `[ok ] tools.good`, `[WARN] tools.typo`
  ("not runnable"), `[ok ] tools.remote`.
- Real models, both protocols: `python3 review/dogfood/mcp.py [--provider kimi]` — the server's log shows
  `initialize`/`tools/list`/`tools/call`, and the run reports the tool's own output (a token the server
  generates at start, so a model that answered from the tool's *description* instead of calling it cannot
  pass): deepseek 2.5 s, kimi 9.6 s, both `end=reply` with the token.
- The pre-fix behaviour, measured: a real session with the same config started the daemon and never spawned
  the server (no marker file, no `tool service` line), while the model's request went out without the tool.

Ceiling: a declared service is bound to **every** member (the bindings list is per session, not per member) —
per-member MCP selection would be new surface and has no user request behind it. And an MCP server is started
with the environment the tool gateway builds (its own `env` table plus the documented references), not with
the daemon's ambient environment — worth knowing when a server expects a variable to be inherited. No new
formal claim came with this change: an MCP tool reaches a model only through `BoundTools::schemas()`, whose
only inputs are the trust-filtered catalog and the bindings list, so the design's "binding is the
authorization" (§12.1) still holds by construction, and `V2Grants::OfferedToolsAreAuthorized` continues to
cover the grant-backed half of the offered surface.

## D-73 The CLI refuses what it does not honour (2026-09-25)

The documented-surface audit that produced D-71/D-72 turned to the entry points themselves, and found the same
class of defect in the argument parser: **arguments accepted with nothing behind them**.

| Input | What happened | What happens now |
|---|---|---|
| `teamagents hello` | `hello` was parsed as a positional, the top-level dispatch fell through to `run_tui`, and a session was booted with the word silently dropped — a typo'd verb (`teamagents exex "…"`) or a pasted prompt lost exactly what the user meant | exit 2, the message names the word, says the TUI takes no prompt, and points at `teamagents` / `teamagents exec "…"`; nothing is started |
| `teamagents frobnicate` | same fall-through (a stray word is not a verb) | same refusal |
| `teamagents -v` / `--verbose` | the flag was parsed into a field no code ever read: verbose logging exists in no release this binary serves | exit 2, pointing at `<state root>/daemon.log` and naming `--version` (the plausible `-V` typo) |
| `teamagents-tui --cwd DIR` | the TUI accepted `--cwd`, `--full-auto`, `--resume` and `--team` and honoured none of them (the engine passes the socket and the state root; the session's workspace and mode belong to the daemon) | exit 2 with the flag named and the pointer to `teamagents --cwd DIR` / `teamagents --full-auto`; the engine no longer passes `--cwd` through |
| `teamagents` on a machine with no config | the daemon refused with `use --model to name a catalog profile (available: )` — a first run misreported as a missing flag | the message names the step that creates a catalog (`teamagents init`, then `doctor`) |

The rule this restores is the one the upgrade notes already state for the *removed* entry points
(`--plain`/`--resume`/`--team`, `validate`/`sessions`/`serve`/`repl`): an argument this binary does not serve
fails with a clear message instead of being ignored. It matters more here than it looks: the TUI path has a
**side effect** (it boots a daemon and opens a session), so silently dropping an argument also wasted the
user's session on the wrong work.

Evidence: `cli::a_bare_word_and_verbose_are_refused_without_starting_a_session` drives the real binary for
`hello`, `frobnicate` and `-v` (exit 2, the message names the input, and — the part that matters —
`<state root>/daemon.sock` was never created); `tui::cli_flags::the_tui_refuses_the_flags_the_daemon_owns`
drives the real front-end for `--cwd`/`--full-auto`/`--resume`/`--team` and shows a supported invocation still
parses. `make pty` keeps driving the real terminal through the engine (which no longer passes `--cwd`), and
`make check` is green.

Ceiling: `--help` after a *known* verb still prints the global help rather than per-verb help (the usage
lines are in it, so nothing is misleading); and the two flags of the removed `sessions` verb (`--dry-run`,
`--history-days`) are still parsed so that `teamagents sessions prune --dry-run` reaches the "no longer
supported" pointer instead of a bare usage error.

## D-72 A run reports its own input's outcome, also when that input waited (2026-09-25)

D-71 taught the client to stop reading the runtime's word as the member's answer. The next audit of the same
surface — the D-63 path where `exec` arrives while the leader is already in a turn — found the same defect one
step earlier, and this time in the *primary* outcome:

```
$ teamagents exec "second question"      # submitted 1 s into the first run's slow turn
exit=0 end=completed goal=SUCCEEDED reply=null input_queued=true
```

The prompt was queued (D-63 reported that honestly), the *first* run's turn then settled the goal, and `exec`
reported **that settlement as its own outcome**: exit 0, "completed", no answer to the question it was given.
The same shape applies to a plain reply: while the queued input waited, the earlier turn's reply was the
newest assistant entry, and a poll that landed in the boundary between that reply and the drain reported it
as this run's answer. D-49's contract says "own outcome only"; it was written for a settlement left by an
earlier *run*, and the D-63 queueing path had no equivalent rule at all.

**The rule.** A turn-ending entry is this run's outcome only if it comes *after* this run's own input entry in
the conversation. `exec` knows that entry exactly, because it generated the envelope id, and the daemon's
history view now names it (`envelope_id`, D-72 also fixes the page's order to (epoch, idx), which is the order
the conversation actually has across a reset). From one history snapshot the client takes its own position and
then reads the **first** turn-ending entry after it — the member's text (no pending tool calls) or the
runtime's closing word: that is its answer, and a later turn's entries are not. A settlement is recognised by
the runtime's own note (`goal-close-*`, `goal-block-*`), whose position after this run's entry is exactly the
fact "it happened after my input landed" because the note is appended in the settlement's transaction.

**What the rule is allowed to assume, and why it is verified.** A queued input is applied at a READY boundary
and the next request is fixed from that context (§5.3), so a turn begun after the landing *contains* the
input, and an outcome recorded before the landing cannot belong to it. That premise is a model property, not a
hope: `InputLandsAtTheBoundary` (D-63) plus the boundary's own rule that a request may not begin while the
inbox holds something (`~inst[i].queue`, whose counterfactual `MC_control_midturninput.cfg` is refuted). D-72
adds the property that names the consequence: **`SettlementFollowsATurnAfterTheLanding`** — at settlement time
a request must have begun since the instance's last input landing. It is refuted by the same
pre-D-63 counterfactual in its own control (`MC_control_landing.cfg`, `AllowMidTurnInput = TRUE`), whose
counterexample walks `BeginRequest → MidTurnInput → RecordAttempt → ImportResponse → SettleGoal` and shows the
settlement of a turn that never saw the input — precisely the outcome a waiting client would have attributed
to it. The model also gained the abstraction it was missing for this: `SettleGoal` now requires
`tail = "assistant"`, i.e. a settlement is about the model's own completion, which is what both code paths do
(`complete_goal` reads a decision's candidate; a runtime block follows the check round of one).

**An input that will never land says so.** A queued envelope whose epoch closes before the boundary reaches it
is sealed as `SUPERSEDED` (§5.3/A24) — a reset sealed the one in this audit's probe. The runtime names what it
seals (`envelopes_sealed`, from both places that seal: the drain dropping a stale-epoch leftover, and
`close_epoch_execution` closing an epoch), and `exec` ends at once with the new terminal
`end: "undelivered"` (exit 1) instead of letting the caller wait out its own deadline and then call a dropped
input a timeout.

**Evidence** (all re-runnable):

- `v2_daemon::a_queued_input_is_not_answered_by_the_previous_turns_settlement` — a real socket, a slow
  settling turn, the second input queued behind it: the run reports `end: reply` with **its own** answer,
  `goal_status: null`, `input_queued: true`, exit 0, while the session's goal really is `SUCCEEDED`. With the
  pre-fix decision restored the same test fails (`left: Completed, right: Reply`).
- `v2_daemon::a_queued_input_is_not_answered_by_the_previous_turns_reply` — the positional half of the rule.
- `v2_daemon::a_queued_input_a_reset_sealed_is_reported_undelivered` — a reset while the input waits: exit 1
  with nothing claimed as its outcome, instead of a 30 s wait for a timeout.
- `v2::exec::tests::an_outcome_before_the_runs_own_input_is_not_its_outcome` — the rule as a pure function
  over the shapes a conversation can have (unlanded input, earlier settlement, tool traffic, a later turn,
  the run's own settlement, a closed turn, a bare tool call).
- Real models, both protocols: `python3 review/dogfood/queued_input.py [--provider kimi]` — run 1 settles its
  own goal, run 2 is queued inside it and reports `end=reply` with its own word (`BANANA`) and
  `goal_status: null` (deepseek 2.8 s, kimi 40.4 s). Against the pre-fix build the same harness fails with
  `end=completed / goal=SUCCEEDED / reply=null` for the queued run.
- Formally: `SettlementFollowsATurnAfterTheLanding`, listed in `MC.cfg` and `MC_control_two.cfg`
  (`make verify-model-all` green: MC.cfg 84,877 states, MC_control_two 958,777), and refuted by
  `MC_control_landing.cfg` in `make verify-model-counterexamples` (now 9 controls, each refuted).

Ceiling, stated honestly: the client's rule is positional over the conversation, so it inherits the history
page's bound (`exec` reads 400 entries) — a run whose input entry has already fallen out of the page cannot
be attributed, and `exec` then reports a timeout rather than inventing an outcome. The queue is still
per-instance and per-epoch: an input sealed by a reset is *reported*, never re-delivered, and re-sending it is
the caller's decision. And the *session-level* facts stay ungated on purpose: a permanently failed leader
request and a pending approval are reported even when they belong to the turn the input waited behind, because
they are why this run cannot deliver anything — the report carries the reason and the exit code is non-zero
(1/3), so nothing is claimed as this input's own outcome.

## D-71 A runtime-blocked goal is not a reply: the runtime's own word is never the member's (2026-09-25)

A16's harness says a required check that can never pass must end the run **failed with the goal BLOCKED**.
Against DeepSeek that is what happened (D-70). Against the second catalog entry, Kimi over the `responses`
protocol, the same scenario ended

```
provider=kimi exec exit=0 end=reply goal=None | goal BLOCKED | check rounds 3 | repairs 2
```

— the goal *had* been blocked, and the headless run reported a **success with no goal status**: a false
success in the exact place the gate exists to prevent one. Two defects compounded:

1. **The block was not a settlement.** `complete_goal` announces its outcome with a `goal_completed` event;
   `block_goal` wrote the goal status and emitted only `goal_blocked`. A client that follows a goal's ending
   through the event log — the headless run, the TUI — never learned that the goal ended, so `exec` fell
   through to "the last entry is the reply".
2. **The runtime's own closing note was stored as the member's.** All three runtime notes
   (`runtime: goal … blocked: …`, `runtime: turn closed`, `runtime: goal … closed as …`) were context
   entries of kind **`assistant`** with an assistant-role message. The client's last resort — read the last
   assistant entry as the model's answer — therefore returned the runtime's *own* sentence
   (`runtime: goal goal-s-main blocked: required checks failed (impossible:exit) after 3 round(s)`) as the
   reply, and `exit 0`. The note was written that way on purpose (a note that looks like model text keeps
   the driver idle), which is exactly why the confusion was invisible: the code had one word for two
   speakers.

**The fix, in three places, each with the reason it is the right level:**

- `block_goal` emits the same `goal_completed` event as `complete_goal`, with `status: "BLOCKED"` and
  `blocked_by: "runtime"` (the distinct `goal_blocked` event stays: it carries the check names and reason,
  and keeps "who decided" auditable). A settlement is a fact of the event log, not only of a snapshot a
  polling client happens to read at the right moment.
- The runtime's closing notes get their **own entry kind** (`EntryKind::Runtime`, `kind = 'runtime'`) in the
  **user's voice** (`role: "user"`, the same voice the existing `note` kind uses for runtime-authored
  facts). The kind is the discriminator every reader already uses (`exec` reads entry kinds, the TUI switches
  on them), so no client has to pattern-match `"runtime: "` text to tell the runtime apart from the model.
  The idle rule counts the committed tails — the model's own text **or** the runtime's closing word — as
  answered, which is what the assistant-shaped note was for: a runtime that re-opened a turn against its own
  settlement would be inventing work out of its own ending (formally: `RuntimeTailIsWork`).
- `exec` reports what actually happened. A turn the runtime closed with **no settlement this run can claim**
  (the goal settled in an earlier run, or the `finish` had nothing left to settle) is the new terminal
  `end: "unsettled"`, exit 1 — not a `reply` whose text the member never said, and not a timeout that would
  mislabel a finished turn. The loop reads the checkpoint **before** the events for this: a settlement
  commits its event and the phase change in one transaction, so that order cannot see an idle instance whose
  settlement is still unread.

**Sessions written before the fix are migrated, not left behind.** Schema 2 → 3 rewrites exactly the three
runtime envelopes (`goal-close-*`, `goal-block-*`, `turn-close-*`; a model entry carries its decision id
there, never one of these) from `assistant` to `runtime` with `role: "user"`. The migration walks any older
store up step by step in one transaction (a `1`-stamped store still reaches the current version), which the
previous one-step rule did not allow. Probe on a real pre-fix root (`/tmp/ta-providers-run`, written by the
previous build): before `schema_version = 2`, `i-leader:0:13 kind=assistant {"role":"assistant", …}`; after
one daemon boot on the new build, `schema_version = 3`,
`i-leader:0:13 kind=runtime {"role":"user","content":"runtime: goal goal-s-main closed as SUCCEEDED"}`.

**Evidence** (all re-runnable):

- `v2_daemon::a_runtime_blocked_goal_is_not_reported_as_a_reply` — the whole scenario through a real socket
  (a check that always fails, three repair rounds): `end: failed`, `goal_status: BLOCKED`, `reply: null`,
  exit 1, the `goal_completed` event present with `blocked_by: runtime`, and the instance's tail entry of
  kind `runtime`.
- `v2_daemon::a_turn_closed_by_the_runtime_without_a_settlement_is_not_a_reply` — the second run on a settled
  goal: `end: "unsettled"`, exit 1, returned at once instead of waiting out its 30 s deadline.
- `v2_driver::required_checks_exhausted_parks_the_goal_blocked` (updated: the block *is* a `goal_completed`
  settlement now), `core::v2::control::closing_a_turn_answers_its_finish_call` (the close marker is a
  `runtime` entry in the user's voice), `v2::store::migrate_rewrites_the_runtimes_closing_notes`,
  `tui::the_runtimes_closing_note_is_not_the_members_message`.
- Real models, same harness, both protocols: `python3 review/dogfood/checks.py --provider deepseek`
  (exit 1, `end=failed`, goal BLOCKED, 11 requests, 9.7 s) and `--provider kimi` (exit 1, `end=failed`, goal
  BLOCKED, 8 requests, 37.8 s) — where the Kimi run had reported `exit 0 / end=reply / goal=None` before.
- `python3 review/dogfood/providers.py` (A27, two providers) still completes: exit 0, goal SUCCEEDED, the
  delegated task SUCCEEDED, both members on their own model, 8 requests, 23.1 s.
- `python3 review/dogfood/runtime_note.py --providers deepseek,kimi` — the note is not only *stored* under
  its own kind, the transcript still works: two turns in one state root, the first settling `SUCCEEDED` and
  the second (whose request carries the runtime's user-role note) answering normally with exit 0. deepseek
  1.6 s then 0.9 s, kimi 8.9 s then 11.8 s, the note at `i-leader:0:4` as `runtime`/`role: user` in both.
  This is the check the shape needed: `materialize` sends `entry.message` verbatim, so the change had to be
  safe on the DeepSeek thinking wire and on Kimi's `responses` wire, and it is.
- Formally: `NoTurnWithoutWork` now says *no turn while the tail is committed* (the model's text or the
  runtime's closing note), `SettleGoal` leaves `tail = "runtime"`, and the new negative control
  `MC_control_runtimeTail.cfg` (`RuntimeTailIsWork = TRUE`) is **refuted** by that invariant with
  `tail = "runtime"` and `phase = "MODEL_PENDING"` in the counterexample state. `make verify-model-all`
  (10 configurations), `make verify-model-counterexamples` (8 controls, each refuted) and
  `make verify-kani` are green.

Ceiling, stated honestly: the kind and the voice are the fix for *new* sessions and the migration covers the
old ones, but a client that keys on text rather than kinds would still be reading a convention. The Kimi
harness also showed the D-65 ceiling from the other side, unchanged by this decision and worth knowing: when
the *worker* answers with prose instead of calling `finish`, the delegated task stays RUNNING, and a leader
that verifies the artifact itself (as it did here — it read the worker's response and the file) can settle
the goal SUCCEEDED with that task still open, because `complete_goal` checks open **operations**, not open
**tasks** — which is what §4.2's completion transaction names, with the delegator's own `wait` as the
mechanism that should hold a leader until its delegated work resolves. Whether the runtime should refuse such
a settlement anyway is a design question about team semantics for the user, not a defect this entry fixes.

## D-70 The completion gate works on a thinking-mode provider (2026-09-25)

Closing A16's last gap — a *real-model* run whose required check fails — found a defect that no deterministic
test could: the check round's own conversation entry is a **runtime-authored assistant message with tool
calls** (`register_check_runs` appends `"runtime required-check round N for goal …"` plus the shell calls,
because the check outputs must answer a tool call to be wire-valid), and DeepSeek's thinking mode requires
such a message to carry `reasoning_content`. It did not, so the *next* request — the repair turn that the
completion gate opens after a check fails — was rejected outright:

```
chat API 400: The `reasoning_content` in the thinking mode must be passed back to the API.
```

In other words: on the default provider, **any goal whose required check failed died on the wire instead of
being repaired or blocked** — the gate was unverifiable in exactly the case it exists for. A16's row had
recorded an earlier wire error there (D-54 fixed the unanswered `finish`); this was the second half.

**Pinning the rule with a replay probe** before touching code (same session, same messages, four variants
sent to the API): as the failed run had it → `400 reasoning_content …`; with a labelled filler on the
synthetic entry → `200`; without the synthetic assistant message (orphan tool result) → `400 Messages with
role 'tool' must be a response to a preceding message with 'tool_calls'`; with no reasoning anywhere →
`400`. So the transcript shape is forced by the wire (the synthetic call must exist), and the missing field
is the defect.

**The fix lives in the adapter, not in the stored context**: `ChatCompletions::with_reasoning_echo` (set for
the `deepseek` protocol in `build_for_model`) fills an **empty** `reasoning_content` on assistant messages
that carry tool calls and have no recorded reasoning. An empty value satisfies the requirement and invents
nothing; the probe also verified it is accepted by `deepseek-flash`, `deepseek-reasoner` and `deepseek-chat`,
so the scope is safe for every model on that protocol, and a *recorded* reasoning is never overwritten.
Keeping the field out of the context preserves the design's protocol-neutral kernel (native continuation
fields stay per-protocol, as the existing `responses_output`/`anthropic_blocks` stripping already does).

**Verification**: `providers_fake::the_thinking_wire_echoes_reasoning_for_assistant_tool_calls` (the filler
appears only when the flag is on, and a recorded reasoning is untouched), and the real-model harness
`review/dogfood/checks.py` that found the defect now completes: 8 model requests, 12.7 s, `end=failed`,
goal **BLOCKED**, the artifact written, the repair ledger naming `check_id: impossible` / `class: exit`, and
the model itself reporting that it would not bypass the gate.

Ceiling: the *protocol* decides the echo (`deepseek`), not the model name; a non-thinking DeepSeek model on
that protocol receives an empty field it ignores (verified), and a thinking model served over the plain
`openai` protocol would need the same treatment — the swap is the flag.

## D-69 Which model a member runs on is written down and visible (2026-09-25)

Making the two-provider acceptance row (A27) re-runnable in the tree — a real DeepSeek + Kimi session, below —
surfaced a visibility hole. A spawned child stores its profile when it is created (D-59 made that the
*resolved* model name, so the factory can map it back to a catalog key), but the **leader's** row was created
by the bootstrap with no profile at all: `instances.profile_json` was `{}`, and no surface reported a model
anyway. For a team that deliberately spans providers (`spawn(model = …)`) that means the user cannot see who
runs on what — in the TUI, in the daemon's snapshot, or in the CLI listing.

**The fix** (three small pieces, no new mechanism):

- `driver::bootstrap` stores the leader's resolved profile (`model`, `instructions`, `options`,
  `context_window`) on the instance row, so *every* instance row describes its model. The leader's driver
  still takes its profile from the session configuration — the row is what a reader has. The bootstrap's
  idempotence is unaffected: it only creates the row when the instance does not exist yet, so a fixed
  `boot-instance` command id never replays with a different payload (the check happens before the submit).
- The daemon's `checkpoint` snapshot carries `model` per instance (a read-view extension, like D-61's grant
  view), so **every** client sees it.
- The TUI instances panel and `teamagents instances` print it (`i-worker · ACTIVE · READY · k3-256k`).

**Evidence**: `v2_daemon::the_snapshot_reports_each_members_model` (the row and the snapshot carry the
*resolved* name `deepseek-flash`, not the catalog key `leader_main` — consistent with what a spawned child
stores) and `tui::frame_shows_the_panels_and_panel_hit_testing` (the panel renders each member's model).

**The re-runnable A27 harness** is `review/dogfood/providers.py`: one session, the Leader on DeepSeek Flash
(native 1M window, D-36) and a worker spawned with `model = "worker_kimi"` (the user's Kimi entry, 262,144
tokens), delegating a file write and waiting for it. Measured (2026-09-25, isolated state root): 7 model
requests, 14.0 s, `end=completed`, goal `SUCCEEDED`, members `i-leader` on `deepseek-flash` and `worker1` on
`k3-256k`, `task t1 SUCCEEDED`, and `answer.txt` exactly as asked. A27's row now cites this script instead of
an out-of-tree probe.

## D-68 The user-side interventions are reachable headlessly (2026-09-25)

D-65's honest ending for a model that stops talking — the delegator waits, and **cancelling the task** releases
it — was reachable only from the TUI, and the same is true of the other §5.4 levers: `exec` tells a stuck user
to "resume it in the TUI instances panel (r)", and the operating notes send the user to the tasks panel to
cancel a task that can only wait. A headless or CI user has no panel, so the documented recovery paths were
unreachable for exactly the users `exec` exists for.

**`teamagents tasks [list] [--json]`, `tasks cancel --id ID`** and
**`teamagents instances [list] [--json]`, `instances pause|resume|terminate --id ID [--yes]`**
(`engine/src/v2/intervene.rs`) close that: they are clients of the ordinary user commands
(`cancel_task`, `set_lifecycle`) whose identity rules the control plane already enforces — *the user controls
every transition; the system may only park* (§5.4) — with the same exit codes and prefix resolution as
`authority`/`approvals`.

Two deliberate choices: `terminate` requires **`--yes`**, because termination retires the instance's
workspace (a directory with uncommitted or unmerged work is never deleted — only reported) and the TUI asks
for a confirmation for the same reason; and the ids must name a *listed* instance or task, so a typo is a
client-side refusal instead of a guess about something else.

Evidence: `v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance` drives the real
binary against a real daemon through the whole D-65 flow — the worker's prose leaves the task `RUNNING` and
the leader `WAITING`, `tasks` lists it, `instances pause`/`resume` move the lifecycle, `terminate` is refused
without `--yes`, `tasks cancel` cancels it, and the delegator wakes and settles the goal `SUCCEEDED`. That
test is also the correspondence for a wait fact D-65 depends on: a `CANCELLED` task *satisfies* a delegator's
task wait (the code's task condition accepts `SUCCEEDED|FAILED|CANCELLED`; a `BLOCKED` task does not).

Ceiling: no bulk levers (`cancel --all`, `pause --all`) and no "send a message to a worker" verb — each
action stays one named subject, and the TUI remains the surface for browsing a large team.

## D-67 Approvals are reachable headlessly (2026-09-25)

Dogfooding the documented first run (`init → doctor → exec`) in a clean `HOME` with a real model ended at a
dead end: the leader's first out-of-scope call parked the session on an approval, `exec` reported
`end: approval_required` and exited **3** — and its message said the only ways on were the TUI or
`--full-auto`. For a *headless* user (the reason `exec` exists, and what a CI job can actually run) that is a
wall: the session is parked, the TUI is not available, and `--full-auto` changes the permission mode of the
whole session.

The mechanism was never missing — this is D-61's shape once more. The daemon's `approvals` read already
carries the **id**, the operation, the tool and a bounded preview (so a client could always have decided,
unlike the grants view D-61 had to extend); `approve`/`deny` are ordinary user commands; and the decision is
bound to the operation and its argument hash, so a modified call needs a new decision (§6.2). The gate itself
is verified: `V2Control::NoEffectBeforeApproval` (± `A25`'s tests) — what was missing was a client.

**`teamagents approvals [list] [--json]`, `approvals approve --id ID`, `approvals deny --id ID`**
(`engine/src/v2/approvals.rs`): a client of the same socket, taking the full id or an unambiguous prefix
(the resolution rule now lives once, in `exec::resolve_prefix`, shared with `authority`), with the same exit
codes as `authority` (0 done, 1 the session refused it, 2 usage/no session). The listing names the exact call
being approved; a deny prints that the operation fails closed.

Real-model evidence (one shell, clean `HOME`, the shipped config, DeepSeek Flash): `exec` parked on
`printf hi > shellproof.txt; echo "exit=$?"; …` → `teamagents approvals` listed
`ap-d-req-4caeb0c6-…:0  shell  printf hi > shellproof.txt; …` → `approvals approve --id ap-d-req-4caeb0c6`
→ the session dispatched the approved call, the **goal reached `SUCCEEDED`**, `shellproof.txt` contained
`hi`, and the list was empty afterwards. Deterministic evidence:
`v2_daemon::the_approvals_cli_lists_and_decides_a_parked_operation` drives the real binary against a real
daemon socket (listing, prefix decision, a typo refused as a client error, the goal completing after the
decision).

Ceiling: the decision is still one *visible* call at a time — there is no "approve everything like this" rule
and no interactive prompt, because the design binds a decision to one operation and its arguments (§6.2); a
user who wants unattended runs still chooses `--full-auto` deliberately.

## D-66 The doctor reports the skills registry, so a bad path is not silent (2026-09-25)

Auditing the documented configuration surface against the code found a silent trap: `skills_paths` and
`instruction_files` are read from the user config, `expand_home` resolves `~/…`, and
`tools::skill_roots` *filters out* a configured path that is not a directory — but the only validator
(`config::validate_configured_paths`) is called from `load_user_config_for`, the **project** config loader
that no product entry point uses yet (the known gap recorded in `docs/ACCEPTANCE.md`). So a typo'd path, or a
`~/.agents/skills` that does not exist yet, meant: skills silently absent (`skill` answers "no skills
configured" only when the model happens to ask) and instruction files silently missing from every prompt,
with nothing anywhere telling the user.

`teamagents doctor` now reports both: `[ok  ] skills  1 skill(s) under 1 configured root(s)`,
`[WARN] skills  a configured root does not exist and is ignored, so those skills never load: ~/.agents/skills`,
`[WARN] skills  none configured: skills_paths in the user config registers a root …`, and an
`instruction files` row for the same reason. Verified on a clean first run
(`HOME=/tmp/fresh-home XDG_CONFIG_HOME=… teamagents init && … doctor`): the shipped config registers
`~/.agents/skills`, which does not exist on a fresh machine, and that is now visible as one WARN instead of a
skill list that is quietly empty.

It is a **warning, not a load failure**, deliberately: the shipped `init` config points at the documented
registration root (D-34), and refusing to start a session because a *feature's* root is missing would break
the documented first-run flow (`init → set the key → doctor → teamagents`) on every machine that has not
created it yet. Refusing a bad path stays the behaviour of the project-config loader once that loader is
wired (a decision that needs the user's word).

Evidence: `cli::doctor_reports_the_skills_registry_and_missing_configured_paths` (a root with one skill
reports `1 skill(s) under 1 configured root(s)`; a missing root and a missing instruction file are named;
no configured root says where to put one and adds no instruction-files row) plus the clean first-run probe
above. `make check`, `make pty` and `make verify-model-all` are unaffected and green.

## D-65 A plain reply opens no further turn: the turn storm is over (2026-09-25)

The probe's runaway had a root that is not a policy question after all. The driver's idle rule was
"the last entry is the model's own text **and** no open tasks" — so an instance that still owed a task was
asked again after every reply. A model that answers with prose instead of settling the task (the probe's
worker said `BLOCKED.`) was therefore asked forever: **169 model requests / 1,226,717 prompt tokens / 181
context entries in ~15 minutes**, no progress, bounded only by a goal budget that the session did not have
(D-64 now lets the user configure one, which is a ceiling, not a fix).

Two things say this was a defect rather than a design choice:

- §3 states that a **plain reply settles no task and no goal** — a turn without tool calls is the model
  saying it is done for now. Re-opening a turn against that is the runtime inventing work.
- `V2Control`'s `NoTurnWithoutWork` — *the already-verified model property* — forbids exactly the state the
  clause produced: `MODEL_PENDING`/`TOOLS_PENDING` while the last word is the model's own. The code
  deviated from the verified model, and the model's counterfactual switch (`ReaskAfterReply`) now proves it:
  with the old clause enabled, TLC reports **`Invariant NoTurnWithoutWork is violated`**.

**The fix**: the idle rule counts only *unaddressed* work — content that arrived since the last request (the
boundary drain applied it, D-63) or a task this instance has **not started yet** (`PENDING`). A turn whose
model replied with prose now ends the instance's activity with the task still `RUNNING`. Nothing is invented
about the outcome (§8: the runtime never reads an outcome out of prose) and the loop stops.

**Who resolves it, and why that is the honest ending**: the delegator's `wait` on the task stays pending
(the TUI shows the instance `WAITING` and the task `RUNNING`), and the user has the documented lever: cancel
the task (`c` in the tasks panel, `cancel_task` in the protocol), which **satisfies** the delegator's wait —
a `BLOCKED` task would not, because the wait's task condition accepts `SUCCEEDED|FAILED|CANCELLED` only. So
cancelling wakes the delegator, which can re-delegate or settle honestly; the session is never stuck on a
model that stopped talking, and it never burns a budget doing it. Parking the *task* `BLOCKED` on the
runtime's own initiative was rejected for exactly that reason and because §5.3 reserves `BLOCKED` for a
closed set with no runnable path (the assignee is still runnable — the user can send it work).

Evidence: `v2_supervisor::a_prose_reply_leaves_one_turn_and_the_delegator_resolves_the_task` walks the whole
flow — the worker runs **one** turn and stays `READY`, the task stays `RUNNING`, the leader is `WAITING`, the
user cancels the task, the leader wakes and the goal settles `SUCCEEDED`. With the pre-fix clause restored the
same test fails (`left: Some(4), right: Some(1)`: four requests in 1.5 s). Formally,
`MC_control_reask.cfg` (switch `ReaskAfterReply`) is refuted by `NoTurnWithoutWork` and is part of
`make verify-model-counterexamples`; `make verify-model-all` stays green (7 configurations + the two
two-instance ones).

Ceiling: a session whose model stops settling tasks now *waits* where it used to spin — the delegator's wait
is visible but not self-resolving, so a user who never looks will see a parked turn (and, with `[limits]`
configured, a parked goal). That is the deliberate trade: a visible wait costs nothing, and the alternative
is an unbounded spend. Whether the *runtime* should also offer a bounded "no progress" path (cancel or park
the task after N unsettled turns) remains the user's decision; it is not implemented.

## D-64 The user can bound a goal's cost and time (2026-09-25)

The same audit that produced D-61 and D-63 found the third "designed but unreachable" surface: §8/A18/A35
give a goal a usage ceiling (`limits.max_total_tokens`) and an absolute `deadline`, the control plane really
enforces both (`begin_request` refuses past either and the driver parks the instance with the reason), the
properties are verified (`ReservationsAdmitted`, `AdmissionGate`, `NoRequestAfterDeadline`) — and **no user
surface could set them**. `create_goal`'s limits come from the user config, which carried only `[[checks]]`,
so by default a session ran until the user stopped it. The authority probe's runaway (169 model requests /
1,226,717 prompt tokens, ACCEPTANCE's known gaps) is what made that concrete.

**The surface** is a `[limits]` section in the user config:

```toml
[limits]
max_total_tokens = 2000000   # optional: usage ceiling for every goal this session creates
deadline_minutes = 45        # optional: wall-clock ceiling, counted from goal creation
```

- `max_total_tokens` travels inside `create_goal limits` verbatim, because the core enforces it from there
  (A18). `deadline_minutes` cannot: the core takes an *absolute* timestamp, so `driver::bootstrap` converts
  the duration when it creates the goal and never stores the key on the goal — what is stored is exactly what
  the runtime enforces.
- A zero for either is a config error at load time (it would mean "no request ever"), reported by `doctor`
  and every entry point; like `[[checks]]` this section is **user config only**, never project config.
- `doctor` reports both (`goal limits`), including the honest case "none: a goal (and the session) runs until
  you stop it or the budget is reached".

**Verification.** `config::tests::user_limits_bound_every_goal` (the shape, `{}` when unset, zero refused,
unknown keys rejected), `cli::configured_limits_reach_the_goal_and_really_bound_the_session` (the real daemon:
the goal carries the ceiling, its deadline is ~15 minutes out, the duration key is *not* stored, doctor
reports all three cases) and
`cli::a_tiny_configured_ceiling_parks_the_session_instead_of_running_it` (a 4-token ceiling parks the leader
with `goal … budget exceeded: known 0 + reserved 0 + est 2228 > max 4`). The budget gate itself was already
formal (`V2Control`'s `BudgetFits`/`ReservationsAdmitted`/`AdmissionGate`, A18).

**New formal work**: the deadline was *not* modelled before — `begin_request`'s deadline gate (A35) had only
code tests. `V2Control` now carries `goal.deadlinePassed` (an environment action moves the clock past the
deadline; the fact is monotone), `BeginRequest` requires `~goal.deadlinePassed`, and
`NoRequestAfterDeadline` states the gate. `MC_control_deadline.cfg` is the counterfactual (a runtime that
ignores the deadline, switch `IgnoreDeadline`) and `make verify-model-counterexamples` requires TLC to refute
the property there — it does (`Action property NoRequestAfterDeadline is violated`). The refusal's *park*
shares the classified path `FailRequest` already models.

Evidence: the tests above, `make verify-model-all` (green, `MC.cfg` 6 s / `MC_control_two.cfg` 46 s with the
new invariant and property) and `make verify-model-counterexamples` (six controls, all refuted).

Ceiling: the ceilings are per goal and fixed when the session creates it; amending a *running* session's
limits is still not offered (it is new protocol surface, and the TUI shows the same limits it booted with).

## D-63 An input that arrives during a turn enters at the next boundary (2026-09-25)

The audit of the user-facing surfaces turned up a defect in the *inbound* direction: `submit_input` appended
the input to the instance's context immediately, even while a turn was in flight. A model request is fixed
once it is registered (§3/§6.1), so the input could not be part of it — and the turn's own reply then landed
*after* the input, which left the runtime's idle rule (`step_ready`: the last entry is the model's own text
and no open tasks) satisfied. The message was stored, was never acted on, and the model's answer looked like
the answer to it. The user's message silently did nothing, with no error anywhere — the exact class the design
rules out ("user input enters the target instance at a safe boundary", §5.4).

Reproduced deterministically before the fix (`engine/tests/v2_supervisor.rs`, a scripted provider that holds
its first request open): the reply to the mid-turn input was `{"applied": true}` and the instance made
**one** request where two were owed.

**The fix** keeps the input out of a fixed request: while the instance is `MODEL_PENDING`, `TOOLS_PENDING` or
`COMPLETION_PENDING`, `submit_input` inserts the envelope and returns
`{"applied": false, "queued": true, "phase": …}` (plus an `input_queued` event) instead of appending the
entry. The driver's boundary drains the inbox before it fixes a request (§5.3), so the queued input enters the
conversation in sequence order — after the turn's own answer — and gets a turn of its own. `exec` reports
`input_queued` (and says so on stdout instead of pretending the input landed), and the TUI prints
`queued for <id>: it enters when the running turn ends` so the composer's message is visibly on its way. A
reset seals a queued input with its epoch (A24) and a terminated instance cannot take one; a parked instance
keeps it until it is resumed.

**Verification.** `core/src/v2/control.rs::an_input_inside_a_turn_waits_for_the_boundary` pins the three
paths (READY applies, a turn in flight queues and leaves the phase alone, the drain applies it exactly once
and last); `engine/tests/v2_supervisor.rs::an_input_arriving_during_a_turn_enters_at_the_next_boundary` walks
the real driver: the queued reply, the second turn, and the entry order (the input's index is greater than
the first reply's).

Formally, `V2Control` gained the queue (an instance field, set by `QueueInput` while a turn is in flight, and
cleared by the boundary's `ApplyQueued` / `Input`), a monitor for the landing phase, and two properties:
`InputLandsAtTheBoundary` (an invariant — a user input never lands while a turn is in flight) and
`QueuedInputEntersTheContext` (temporal — a queued input enters the context, unless the instance stops being
active or a reset seals it). `make verify-model-counterexamples` now also runs
`MC_control_midturninput.cfg`, the counterfactual in which the driver applies input mid-turn (the pre-D-63
behaviour): it must refute `InputLandsAtTheBoundary`, and it does
(`Invariant InputLandsAtTheBoundary is violated`).

**A spec correction the two-instance run forced**: `Spec`'s fairness used to be
`WF_vars(\E i \in Instances : Recover(i))`, a *disjunction* over instances. With two instances that lets one
instance be starved forever while the other recovers repeatedly, which is not the system (each instance has
its own driver, and the supervisor drives and restarts them one by one). The new liveness property exposed
it, and the fairness is now per instance: `\A i : WF_vars(Recover(i))`, `\A i : SF_vars(ApplyQueued(i))`
(strong, because a crash loop must not starve the drain) and `\A i : WF_vars(TurnStep(i))` (a turn in flight
eventually ends; `FailRequest` is one of its steps, so an approval wait is covered too). Because the wide
configuration cannot finish in a reasonable time, the fairness check got its own small two-instance
configuration, `MC_control_two.cfg` (same domains as `MC.cfg`, two instances; 1,263,649 states / 165,792
distinct / ~45 s, green, now in `make verify-model-all`), and the older disjunction form is kept as a
counterfactual switch (`PerInstanceFairness = FALSE`) whose configuration
`MC_control_two_disjunction.cfg` **must** refute `QueuedInputEntersTheContext` — it does, in
`make verify-model-counterexamples`.

Ceiling: the queued input waits for the boundary, so a client that sends into a running turn sees its message
acted on only after that turn ends (the TUI and `exec` both say so). Whether the *runtime* should instead
interrupt the turn is the same open question as the re-asking loop (see ACCEPTANCE's known gaps) and is not
decided here.

## D-62 Every request answers every tool call it carries (2026-09-25)

D-61's real-model probe (`review/dogfood/authority.py`) turned up a second defect, in the wire protocol
itself. The worker's first turn ended on an **accepted** `finish`; the runtime answers that call by settling
the turn (or the task) instead of by appending a tool result, so the persisted context keeps an assistant
message whose `tool_calls` are never answered. When that worker was given a new task in the same epoch, its
next request was rejected outright:

```
chat API 400: An assistant message with 'tool_calls' must be followed by tool messages responding to each
'tool_call_id'. (insufficient tool messages following tool_calls message)
```

D-54 fixed the *refused* finish (the runtime now answers it with a receipt); the accepted one, and the
finish that is ignored because it shares a response with other calls (answered by a note, not a tool
message), were still unanswered on the wire — and every later request of that instance failed, which is
exactly the "send a follow-up message" flow.

**The fix** is in the wire projection, not in the stored context: `pair_tool_results`
(`core/src/kernel/instance.rs`) moves an existing answer up next to its call, and now also synthesizes
**one tool-role answer per call the log left unanswered**, naming what it is
(`[no tool result follows: the runtime answered this call outside the tool channel]`). The stored entries are
untouched, no extra model turn is spent, and a call that does have an answer keeps it (the D-54 receipt path
is unchanged).

Evidence: `core/tests/kernel_properties.rs::the_wire_answers_every_call_it_carries` covers the five shapes
(a lone `finish`; two unanswered calls; one answered and one not; a fully answered call — where nothing is
added; an answer that landed behind other entries — moved up), and the exhaustive
`wire_view_is_a_paired_permutation` now asserts that every call in the wire view is answered immediately and
that the only added messages are synthesized answers naming a call the instance really made. Real-model check
(`review/dogfood/authority.py`, second run, isolated state root `/tmp/ta-authority-probe3`): the worker's
first turn ended on an accepted `finish` (entry 18, its task settled `BLOCKED`) and **169 following requests
all imported**, with zero `request_failed` events — the same shape that produced the HTTP 400 in the first run
(which had exactly one, on the worker's first request after the new task).

Ceiling: the synthesized answer also covers a call whose receipt was lost for some other reason — the model
then reads that no result follows instead of the turn dying on a rejected request. The trade-off is stated
in the function's doc comment.

## D-61 The user's authority surface (2026-09-25)

§5.1 makes the user the root of authority, and D-58/D-60 made the *model-visible tool surface* follow the
grants. But no user could exercise that authority: `issue_grant`/`revoke_grant` had no caller outside tests
and the evaluation harness, and the daemon's `grants` view did not even carry the grant's `id` — so no
client could have revoked anything. The visible consequence was that a worker the Leader spawns holds no
`shell@workspace` (§5.1) and nothing in the product could give it one.

**What was added**

- `teamagents authority [list] | grant --subject ID --action A --scope S [--parent G] | revoke --grant ID`,
  a client of the running session's socket (`engine/src/v2/authority.rs`): grants and revocations go through
  `SupervisorHandle::submit_user` like every other business command, so they linearize with driver dispatch
  on the single writer (§9) and the client never opens the database itself. `--json` prints the raw report;
  exit codes are 0 done, 1 the session refused it, 2 usage or no session.
- The guards a human needs and a machine caller does not: the action vocabulary and the pair table live in
  `core/src/v2/capability.rs` (`ACTIONS`, `asks_about`, `authorizes_something`), and the surface **refuses**
  a pair no check asks about (`shell@instance:i-worker` would authorize nothing: never dispatched, never
  offered, never refused) with the scope that action is asked over. A subject that does not exist yet is a
  *warning*, not a refusal — instance ids are chosen by the spawner, so granting ahead of a spawn is
  legitimate and a typo is merely likelier. `revoke` takes the full id or an unambiguous prefix, and `list`
  also prints the session's instances, because a grant's subject is an instance id and there was no other
  headless way to read them.
- The read view gained the fields the surface needs: `daemon.rs`'s `grants` reply now carries `id`, `issuer`,
  `parent_grant_id` and the session's grant revision, and the TUI's topology panel shows the short id next
  to each grant (the TUI already reads the same reply).

**A defect found while wiring it**: the old view read `revoked_at` (a `REAL` column) as an
`Option<String>`, so **one revoked grant made the whole `grants` read fail** with
`Invalid column type Real at index: 1, name: revoked_at`. Reproduced with a standalone rusqlite probe before
the fix. The failure reached the TUI as a lost connection (`DaemonClient::call` clears the connection on any
error), so the grants/topology panel went dead and the client flapped — a documented capability
(`docs/USER-GUIDE.md`'s "revoking a grant removes the tool from the surface") that nobody could have used.

**Verification**: a new TLA+ module, `verification/tla/V2Authority.tla` (+`MC_authority.cfg`, wired into
`make verify-model-all`), models the surface: the user reads the view, writes a grant that the pair table
allows, revokes a grant it can *name*, and the instance's model-visible surface is a **cached** variable that
only its next request refreshes (the code recomputes it per request, `driver::team_kernel`). Properties:
`NoDeadGrantPair` (the surface's table equals the derived table of pairs some check asks about),
`ListedIdsAreUsed`, `EveryLiveGrantBecomesRevocable` (temporal), `AuthorityTracesToTheUser`,
`RevokedStaysRevoked`, `ChildGrantsAreCoveredByTheirParent`, `CascadeTakesTheSubtree`,
`CascadeOnlyTakesTheSubtree` (temporal — the half V2Grants does not state: revoking one grant takes *exactly*
its subtree, never the leader's authority with it), `EffectAtMostOnce`, `OnceStaleNeverExecutes`,
`AuthorizedEffectsOnly` (temporal, with the cached surface), `SurfaceChangesOnlyToTheCurrentEntitlement`,
`StaleSurfaceCatchesUp` (temporal) and `BootstrappedAuthority`. 35,950 distinct states, no error.

Because a property that cannot fail proves nothing, `make verify-model-counterexamples` runs three
configurations that model the plausible mistake and **must** be refuted: `MC_authority_badview.cfg` (the view
without the `id` field — the state of the daemon before this decision) refutes
`EveryLiveGrantBecomesRevocable`; `MC_authority_trustsurface.cfg` (dispatch trusts the cached surface, the
mistake §6.1/A04 forbids) refutes `AuthorizedEffectsOnly`; `MC_authority_stalesurface.cfg` (the surface is
never recomputed) refutes `StaleSurfaceCatchesUp`. The target fails if a control verifies instead.

**Evidence**: `v2::authority::tests::a_revocation_takes_the_full_id_or_an_unambiguous_prefix`;
`cli::the_authority_surface_grants_and_revokes_through_the_daemon` (the real binary against a real daemon:
list carries ids and instances, grant to a spawned worker makes the dispatch question true, dead pairs and
unknown actions are refused, an unknown subject warns, a parent the session does not know is the control
plane's refusal, a derived grant dies with its parent and the dispatch question is false again);
`v2_supervisor::a_users_grant_reaches_the_workers_surface_at_the_next_request` (the granted tool appears on
the worker's next request and a revocation takes it away again — the spec-to-code correspondence of
`StaleSurfaceCatchesUp`); `core/src/v2/capability.rs`'s table test (the pairs the surface accepts are exactly
the ones some check asks); `tui/tests/v2app_tests.rs::frame_shows_the_panels_and_panel_hit_testing` (the
topology line names the id). **Real model** (`review/dogfood/authority.py`, DeepSeek Flash on its native
window, 2026-09-25, isolated state root `/tmp/ta-authority-run`): turn 1 the worker answered *"No — I cannot
run shell commands in the shared workspace. My runtime exposes no bash/exec/terminal tool"* (the §5.1 boundary,
said by the worker itself); the grant went into revision 8; turn 2 the same worker ran the command, reported
exit code 0 and `proof.txt` appeared; the revocation went into revision 11 and left no live shell grant. 13
model requests, both delegated tasks `SUCCEEDED`, no failed request.

**Left open (needs the user's word)**: giving a spawned worker `shell@workspace` *by default* would change
§5.1's spawn contract, and letting the Leader hand out its own shell authority (it holds
`shell@workspace`, and `issue_grant` would accept the narrowing) would turn a boundary the user owns into one
the team manages. Neither is implemented; the surface is the user's.

## D-54 The completion path survives a real model (2026-09-25)

Several things fixed earlier were verified again against a real DeepSeek Flash session (isolated state root, native
1,000,000 context window per D-36, effort `high`). Two of those runs failed in ways the scripted providers
cannot express, because they emulate neither a strict wire endpoint nor a model that forgets a field:

- **The repair turn after a failing check was wire-invalid.** The model's `finish` call is an assistant
  `tool_calls` entry with no operation of its own; on the check path nothing answered it, so the transcript
  sent to the repair turn contained an assistant message whose call had no tool answer following it. DeepSeek
  answered `HTTP 400 … An assistant message with 'tool_calls' must be followed by tool messages responding to
  each 'tool_call_id'` and the run died as a permanent model error. `register_check_runs` now answers the
  dangling call (with `[finish received: required checks round N must pass before the goal can settle]`)
  before it appends the synthetic check entry, so the transcript stays valid on both the repair and the
  blocked path.
- **A `finish` without a usable status produced a false failure.** The kernel mapped a missing or unknown
  `status` to `failed`, the goal closed as `FAILED` although the work was done, and the required checks never
  ran (they only run for a claimed success). The kernel now refuses such a call: it produces no completion,
  reports the problem, and hands the call to the runtime as an ordinary intent, which answers it with
  `finish refused: status is required and must be one of success, blocked, failed …` and continues the same
  turn — the uniform "invalid call → error receipt → model retries" path. A stated outcome (`success`,
  `blocked`, `failed`) is unchanged, and the goal can no longer be settled by a call that states nothing.

The structural rule both fixes protect is now asserted where the scripted providers cannot fake it:
`v2_driver::assert_wire_valid` walks the real transcript and requires every assistant `tool_calls` entry to be
answered before the next assistant message (this is what strict endpoints check), and
`a_finish_without_a_status_is_corrected_in_the_same_turn` requires the correction, the continued turn and the
required check. Both tests fail against the pre-fix code (`assert_wire_valid` reported
`an assistant message followed unanswered tool_calls: ["finish-…"]`; the status test reported the goal as
`FAILED`), which is how they were checked.

Evidence: `core::kernel::tests::a_finish_without_a_usable_status_is_not_a_completion`,
`v2_driver::the_check_repair_path_keeps_the_transcript_wire_valid`,
`v2_driver::a_finish_without_a_status_is_corrected_in_the_same_turn`, plus five real runs recorded under
`review/tmp/d50-live/` (one positive gate run, one 400-error run, one false-`FAILED` run, one approval exit-3
run, and the post-fix re-runs). The scripted tests in this repository accept any transcript, so this is exactly
the class of defect the real-service rule exists for.

## D-60 The tool surface follows the grants, and the authority layer is verified (2026-09-25)

The user's rule — a design addition must be formally verified where it can be — had an obvious gap to close
first: the TLA+ set covered the control plane, artifacts, waits, tasks, compression, the daemon and the
required checks, but **authority had no model at all** (no specs mention grants), and D-58/D-59 had just changed
authority semantics: the session's bootstrap grants and the spawn-derived delegate grant.

**The model**: `verification/tla/V2Grants.tla` (+ `MC_grants.cfg`, wired into `make verify-model-all`) models
where a capability comes from — the bootstrap's default grants, narrowing by an instance (manage covers
message/delegate below it, anything else repeats the issuer's own covering grant), the spawn-derived
`delegate@instance:<child>` and its parent link, revocation with the parent-tree cascade and the revision bump,
the dispatch re-check (a live covering grant *and* a still-current stamp), and the offered surface. Properties:
`AuthorizedEffectsOnly` (temporal), `EffectAtMostOnce`, `OnceStaleNeverExecutes`,
`ChildGrantsAreCoveredByTheirParent`, `AuthorityTracesToTheUser`, `RevokedStaysRevoked`,
`CascadeTakesTheSubtree`, `OfferedToolsAreAuthorized`, `BootstrappedAuthority`.

**Result**: `make verify-model-all` is green for all eight modules; the authority model itself is 1,292,517
states / 178,024 distinct / depth 11 / ~1 minute, with TLC's estimated chance that a fingerprint collision hid
a state at 1.2e-9. `make verify-kani` stays green (3 harnesses). Writing the model also falsified its own first
draft twice (an invariant that was really an initial-state fact; a property that ignored that an effect happens
at a *step*, not in a state) — both recorded in `verification/REPORT.md`.

**What the model exposed in the code**: `OfferedToolsAreAuthorized` — the model-visible surface only offers what
the instance's grants back — is **violated by the runtime**: the profile handed `shell` to every instance, so a
child the leader spawned was offered `shell` while holding no `shell@workspace` grant (§5.1's explicit boundary)
and every shell call came back refused. Offering `shell` unconditionally in the model reproduces exactly that
state (TLC: `Invariant OfferedToolsAreAuthorized is violated by the initial state`), so the property is
sensitive to precisely this defect.

The fix keeps the question in one place: `Control::holds_covering_grant(subject, action, resource)` is the
authority question the dispatch re-check already asks, and `driver::team_kernel` now asks it too, dropping
`shell` from the profile unless the instance holds a covering `shell@workspace` grant. The offered tool is
therefore one the instance can dispatch. The spec-to-code correspondence is
`v2_supervisor::the_offered_surface_follows_the_grants`: the leader is offered `shell`/`spawn` (bootstrap), its
spawned child is offered neither, and the test fails when the filter is removed.

User-visible consequence, stated honestly: a spawned worker works with the file/web/skill tools (they need their
binding, not a grant) and has **no shell** until the user grants `shell@workspace` for it. That is the design's
§5.1 boundary; the surface now says so instead of offering a tool that always failed. Opening that boundary by
default (granting shell to a spawned child sharing the project directory) would change the documented spawn
contract, so it stays a question for the user.

## D-59 A spawned team runs: model resolution, spawn-time validation, and a supervisor that survives (2026-09-25)

With D-58 in place the Leader finally *spawned* — and the session stalled instead: two workers sat `READY` with
`PENDING` tasks while the Leader waited, until the headless client's 900-second deadline expired. The daemon log
had the reason: `panicked at src/cli.rs:333: provider for deepseek-flash: model deepseek-flash is not in the user
catalog`.

The chain: the supervisor hands each driver the *wire-effective* profile (its `model` field is the model name the
provider speaks) while the provider factory receives the catalog *key*. For the leader that separation works,
because the supervisor passes the unresolved profile to the factory. A spawned child, though, stores the
parent's **resolved** profile, so its `model` field holds `deepseek-flash` while the factory looks it up as a
catalog key — and the daemon's factory turned that into a `panic!`. The panic happened inside the supervisor's
discovery loop, so the loop died: no further instance was ever given a driver, the workers never ran, and every
delegating Leader waited for work that could not start. The evaluation harness had never seen it because it does
not spawn through the daemon.

Three changes close this, each with its own test:

- `providers::resolve_model` resolves a catalog reference by **key or by the model name the entry declares**, and
  `build_for_model` uses it (its error now lists the available keys). A child's stored profile is therefore
  bootable, which is what the supervisor's own comment already promised for the leader.
- `spawn` gained an optional `model` (a key or a name). The driver resolves it through the catalog *at spawn
  time*, so an unknown reference is that tool call's error — with the available keys in the receipt — instead of
  a provider the supervisor cannot build. This also makes mixed teams reachable: the README has always promised
  that every instance carries its own model, and until now the Leader could not name one.
- The daemon's provider factory no longer panics. An instance whose model cannot be built gets a provider whose
  every attempt fails permanently with the reason (`AnyProvider::Unavailable`), so the instance parks through the
  ordinary classified path (A07: the request closes, the input stays, the instance parks with the reason) and
  every other instance keeps being driven. Ceiling, stated honestly: that parked worker's task stays open, so its
  delegating Leader still waits for it — only the user can cancel the task or terminate the instance (the
  blocked-task flow in AGENTS.md). With spawn-time validation the path is nearly unreachable, and it now fails
  loudly and locally instead of silently and globally.

Evidence: `providers::tests::a_catalog_entry_resolves_by_key_or_by_model_name`;
`cli::an_unbootable_instance_parks_itself_and_the_session_survives` (an instance created with an unknown model,
given work, parks itself with the reason while the leader stays `ACTIVE`); and a real-model delegation run — three instances, both
delegated tasks `SUCCEEDED`, both `[[checks]]` commands passing in the runtime's check round, the whole flow
`instance_spawned → task_delegated → task_started → wait_satisfied → check_round_registered → goal_completed`
in **16.3 seconds** where the same prompt had stalled for 900 (recorded under `review/tmp/dogfood/`).

## D-58 The session authorizes its Leader, so the team feature exists (2026-09-25)

While preparing a multi-instance dogfooding run, reading the grants of a session the CLI had just started showed
exactly one row: `i-leader` / `shell` / `workspace`. Nothing grants `manage`, `delegate` or `message` anywhere in
the product — `issue_grant` had no caller outside tests and the evaluation harness.

That is not a cosmetic gap, because the model-visible tool surface is *derived* from those grants
(`Driver::team_kernel`: `wait` always, then `send`/`delegate`/`spawn` iff the instance holds
`message`/`delegate`/`manage`). So in every real session:

- the Leader was never offered `spawn`, `delegate` or `send` — the model could not even know it might build a
  team, although its instructions tell it to do exactly that, and the README's promise ("you talk to the Leader,
  and the Leader builds the team on the spot") had no mechanism behind it;
- and every collaboration intent would have been refused by `capability_gap` anyway, because the spawner needs
  `manage@session` (the child's `delegate` grant is derived from it).

Why it stayed invisible: the repo's own acceptance evidence came from tests and the A/B/C evaluation harness,
both of which issue these grants by hand before exercising collaboration (`v2_driver`'s spawn tests do it in
their setup). The fixtures therefore passed while the product could not start a single worker.

`bootstrap` (the user's session bootstrap, which already creates the instance and the goal) now also issues
`manage`, `delegate` and `message` over the session for the leader instance — the authority Q5 and D-42 give the
Leader by default. The grants are issued with fixed command ids and *outside* the "instance already exists"
guard, so the call is idempotent (the command-replay path returns the stored receipt) and a session created by
an earlier build repairs itself on the next start instead of keeping its Leader powerless.

Evidence: `cli::the_daemon_grants_the_leader_the_team_authority` reads the grant table through the daemon's own
protocol after starting it the way a user does; `v2_driver::the_leader_is_authorized_to_build_the_team_by_default`
asserts the *first request's* tool list contains `spawn`/`delegate`/`send`/`wait`, that a scripted `spawn` with a
task really produces `instance_spawned` (`task: true`) without any hand-issued grant, and that the three grants
are session-scoped. Both fail against the previous code (the tool list came back without `spawn`).

## D-57 `--cwd` reaches the session, so the agent works where it was asked (2026-09-25)

The first dogfooding run — this repository's own `edit-integrity` evaluation fixture, copied to a scratch
directory, with the fixture's `checks.txt` configured as a `[[checks]]` entry — reported `SUCCEEDED` and the
artifact verified independently, but it took **60 model turns and 291 seconds** for a one-line INI edit, and
the transcript showed why: almost every turn was spent exploring the *TeamAgents repository* (`review/eval/**`,
`docs/DECISIONS.md`, `engine/src/v2/driver.rs`, the probe's own `session.sqlite`, even `ps` for the harness
processes). It only found the intended file by inferring the probe layout and wrote it by absolute path. Its
workspace was the repository, not the `--cwd` it had been given.

Root cause: `ensure_daemon` passed `--state-root`, `--model` and (since D-55) `--full-auto` to the daemon it
starts, but never `--cwd`, so the daemon — and therefore every instance and tool in the session — inherited
the *client's* process directory. `teamagents --cwd DIR` and `teamagents exec --cwd DIR` silently worked
elsewhere; only `teamagents daemon --cwd DIR` ever honoured the flag. The D-49 `--check` commands were correct
because that workspace is resolved client-side, which is exactly why the acceptance gate passed while the
agent worked in the wrong tree.

- `ensure_daemon` now takes a `DaemonRequest` (state root, model, `full_auto`, `cwd`) and forwards `--cwd`.
- The greeting carries `workspace` next to `permissions`, and the client's note reports any requested setting
  that could not apply to a session that is already running, naming the session's real workspace.
- `exec --json` reports `session_workspace` next to `workspace`: the former is where the session works, the
  latter where its own `--check` commands run, and the two differ when the client joins a live session.
- The greeting's fields live in one `SessionFacts` struct threaded through the accept loop, instead of a
  parameter list that had already grown once (D-55).

The pre-fix run also left the edited fixture (`app.ini` with `retries = 5`) in the **repository root**: the
file it was asked to edit, written into the tree the session had actually been given. (The stray file was
removed once recorded.)

Evidence: `cli::cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one` — the daemon reports the
requested directory, the instance's `workspace_ref` (the root the tools use) is exactly that directory, and a
second client with another `--cwd` is told the live one; without the forwarding the test fails showing the
daemon on the client's own directory. The dogfooding run was repeated after the fix with the same fixture and
configuration: **6 turns and 9.1 s** (from 60 and 291 s), instance workspace correct, artifact verified, and a
second fixture (`rust-fix`, `cargo test` as the check) ran in 8 turns and 8.4 s with its acceptance command
passing both in the runtime's check round and independently (evidence under `review/tmp/dogfood/`).

## D-56 Kernel protocol notes reach the model (2026-09-25)

`interpret_response` produces protocol notes when a response mixes calls that exclude each other — a `finish`
that is not the only call, or a combined `wait` — and `import_interpretation`'s own comment said they "join the
context as their own entry in a follow-up input". They did not: the driver only printed them to the daemon's
stderr, so a model whose completion was ignored never learned why and could repeat the same mistake every turn
(the same channel gap that made a refused `finish` unrecoverable before D-54).

`import_response` now accepts the notes and appends them as one `note` entry (user-role text prefixed
`[protocol note]`, deduplicated by `note-<decision_id>`, atomic with the import so a crash cannot lose them),
and the driver passes `interpretation.notes` with the import. A refused `finish` deliberately carries no note
any more: it rides as an ordinary intent, so the runtime answers that call with the same problem and a note
would say it twice.

The ordering is what makes this safe: `materialize`'s `pair_tool_results` moves every answer next to the call it
answers, so a note written before the decision's receipts still reaches the model *after* them — the order
strict wire endpoints require (D-54).

Evidence: `v2::control::tests::protocol_notes_reach_the_model_after_the_receipts` (the note entry, its dedup
envelope, and the wire order assistant → tool → note built by the real kernel),
`kernel::tests::a_finish_without_a_usable_status_is_not_a_completion` (a refused finish carries no note),
`v2_driver::an_ignored_finish_reaches_the_model_as_a_note` (the model's *next request* contains the note; it
fails without the plumbing).

## D-55 `--full-auto` reaches the session it starts, and the mode is visible (2026-09-25)

A probe run parked on an approval although it had been started with `--full-auto`: both entry points parsed
the flag and dropped it. `ensure_daemon` took only a model key, so `teamagents --full-auto` and
`teamagents exec --full-auto` started a plain `approved_scope` daemon — the documented flag (README argument
table, user guide §4) was a no-op, and the user guide's troubleshooting advice ("or run with `--full-auto`")
did not work either. Only `teamagents daemon --full-auto` ever took effect.

- `ensure_daemon` takes the flag and forwards `--full-auto` to the daemon it starts, and reports whether it
  had to start one.
- The daemon greeting now carries `permissions` (the mode the session booted with). It is additive: clients
  that do not know the field ignore it, and the mode is fixed for the session's lifetime (D-41), so the
  greeting is the honest place to state it.
- Calling `exec`/`teamagents` with `--full-auto` against a session that is already running prints
  `note: a session is already running for this state root in <mode> mode, so --full-auto did not apply …`
  instead of silently pretending. `exec`'s JSON report carries `permissions` as well, so a CI log records
  which mode the run happened in.

Evidence: `cli::full_auto_reaches_a_started_daemon_and_is_reported_against_a_live_one` starts a session
through `exec --full-auto`, asserts the daemon greeting reports `full_auto`, then runs against that live
session and asserts the note names the real mode (and that the daemon it started is stopped again).

## D-53 Documentation claims corrected to the code (2026-09-25)

Continuing the documented-surface audit (D-49 … D-52), three prose claims did not match the implementation. All
three are now written as the code behaves, in both READMEs and in `AGENTS.md`:

- **"an approval … `once` expires after use"** implied an approval *mode* that no longer exists. There is one
  approval action, and the dispatch re-check requires the approval row to be `APPROVED` with exactly the
  operation's `args_hash` and `grant_revision` (plus an optional `expires_at`), so an approval covers one
  dispatch of one operation. The text now says that. (The neighbouring claim — a pending approval of an
  operation that settles, or of a terminating instance, expires automatically — is real:
  `expire_pending_approvals` is called on those transitions.)
- **"file read/write/search with atomic multi-file edits"** overstated `edit_file`, which replaces exactly one
  occurrence in one file (`old_string` must be unique, optional `expected_sha256`); a process-wide write lock
  serializes mutations and writes are atomic per file. The text now describes that instead of promising one
  atomic multi-file edit.
- **"`Enter` send, `Shift+Enter`/`Ctrl+J` newline"** in the key list was aspirational until D-52 made it true,
  so the wording there was fixed together with the implementation (and now names `Ctrl+J` as the chord that
  works in every terminal).

## D-52 The composer really is multi-line (2026-09-25)

Both READMEs (and `tui/src/text.rs`'s own module comment) promised "`Enter` send, `Shift+Enter`/`Ctrl+J`
newline", and `Composer::insert_newline` existed — but nothing called it for *typed* keys: `Enter` submitted
unconditionally, so `Shift+Enter` sent the message, and `Ctrl+J` was swallowed by the "control chords never
insert text" rule. Only pasted text could carry a newline. A coding agent's main input is often a multi-line
instruction, so this was a real gap between the documented interface and the code.

`composer_key` now inserts a newline for `Ctrl+J` and for `Shift+Enter` before the submit arm. `Ctrl+J` is the
universal one: in raw mode the terminal's `0x0A` reaches crossterm's parser as `Char('j') + CONTROL`
(crossterm documents exactly that for `\n`, which is why the chord works in any terminal), while
`Shift+Enter` needs a terminal that reports modifiers — the TUI already pushes the keyboard-enhancement flags,
so modern terminals do. The footer hint now advertises `Ctrl+J newline`, reordered so that a narrow terminal
truncates the least important chord instead of the pending-approvals hint (78 characters with one approval
pending, so it fits an 80-column terminal as well).

Evidence: `v2app_tests::composer_inserts_newlines_and_submits_the_whole_text` (both chords, and a plain
`Enter` still submits the whole text) and — in a real terminal, through the real binary —
`make pty`'s `pty_v2_smoke.py`, which types two lines with `Ctrl+J` in between and asserts that the submitted
input is exactly `"first-line\nsecond-line"`.

## D-51 The coordinator lock absorbs the fork/exec window (2026-09-25)

`make check` failed intermittently (roughly every second run on a loaded machine) with
`state root already has a coordinator: … the operation would block`, always in a test that restarts a
coordinator after a simulated crash (`v2_driver`'s two crash tests, `v2_mcp`'s recovered-dispatch test). The
failure reproduced on unmodified `HEAD` (a pristine `git archive` copy failed the same way), so it was not a
regression from the work in D-49/D-50 — it was a real hazard the tests happened to trip.

**Root cause.** `jobs::state_lock` holds the coordinator lock with `flock` on a `File` opened `CLOEXEC`, which
is what the design means by "never inherited by tool children" (§6.1). But `CLOEXEC` only takes effect at
`exec`: `Command::spawn` (a shell-job runner, an MCP server, a hook) forks a child that, until it execs,
inherits the *entire* descriptor table of the process — including every coordinator lock any thread is
holding. During that window the lock is genuinely alive, so a `try_lock` from another thread or process gets
`EWOULDBLOCK` and the restart reports a coordinator that is about to disappear. Under load the window widens
(the child is not scheduled promptly), which is exactly when the failures clustered; with the whole suite
serialised or the tests run alone, the window never overlapped the restart.

The probe evidence: on a failing run the lock file had exactly one descriptor in the panicking process (the
fresh one) while a *different* process whose `cmdline` was still the test binary held an inherited descriptor
to the same file — a pre-`exec` child. A 25-round sequential `start → crash → start` loop on one state root
never failed, which ruled out a leaked handle; only the concurrent-spawn case did.

**Fix.** `jobs::state_lock` now waits up to two seconds (10 ms steps) for a busy lock instead of failing
immediately, and still fails with the same message once the wait is over. Exclusivity is unchanged — the
kernel grants the lock to exactly one holder, and a genuine second coordinator keeps it for far longer than
the window, so A33's "a second daemon is refused" still holds (it just takes the bounded wait to say so).
A filesystem that cannot lock at all (any error other than `WouldBlock`) still fails immediately, because
that is a hard error rather than a busy lock.

While chasing it, a second, unrelated flake surfaced once in a five-run loop: `budget_exhaustion_parks_the_instance`
and `goal_deadline_parks_the_instance` read the lifecycle once, immediately after the refusal event. The park is
a separate committed step from the refusal, so under load the snapshot could still show `ACTIVE`. Both tests now
wait for the lifecycle they assert on (`wait_lifecycle`, alongside the existing `wait_phase`), which is what they
meant in the first place; the product behaviour was correct.

Evidence: `jobs::tests::a_momentarily_held_lock_is_absorbed` (a short hold is absorbed and the winner waited),
`a_live_holder_is_refused_after_the_wait` (a live holder still loses, with the A33 message),
`the_default_wait_is_bounded`; `v2_driver::a_crashed_driver_releases_its_coordinator_lock` covers the release
contract itself. After both fixes, `make check` ran green five times in a row under exactly the conditions that
failed in roughly half of the earlier runs (four runs before the second fix: no lock failure remained, one
lifecycle-race failure).

## D-50 User-defined completion checks become reachable (2026-09-25)

The design requires that "required checks defined by the user or project must pass" (§8, Q11) and the runtime
implements the whole path — `create_goal limits.required_checks` → the driver registers the check operations
at the completion boundary → a failing check enters repair and finally blocks the goal (A16/A17). What was
missing was any way for a *user* to define them: the goal limits were hardcoded to `{}` in `cli::daemon`, so
in practice every real session settled on the model's own report. D-49 recorded that as the ceiling; this item
closes it with the smallest surface that fits the existing conventions.

- **`[[checks]]` in the user config** (`id`, `command`, optional `timeout`, `network`, `inputs`) is now the
  user's acceptance contract. `config::goal_limits` turns it into the goal limits the daemon boots with, and
  the *same* core validator that guards `create_goal` runs at config load — so the config edge and the control
  plane cannot disagree, and a broken entry (empty command, `timeout = 0`, an escaping input) fails `doctor`
  and every entry point instead of a goal silently never settling.
- **User config only**: `checks` joins `hooks` and `retention` in the list of sections a project file may not
  define. A check runs unattended at the completion boundary without an approval prompt, so letting a cloned
  repository install one would be remote code execution by config. (Wiring the project config into the daemon
  at all is a separate, still-open question; today only the loader knows about it.)
- `doctor` reports how many checks will gate the session and which ids they are, and `[[checks]]` is
  documented in `examples/config.minimal.toml`, `examples/config.toml` and the user guide (§2.1).
- The TUI names the checks of a round and the ids that failed (`v2app::apply_events`), so a goal being
  repaired or parked explains itself while the check receipts show up in the conversation as tool results.
- The headless client's `--check` (D-49) keeps its v1 semantics and is documented as the *weaker*, client-side
  acceptance command: it decides `exec`'s exit code after the turn, while a runtime check prevents the goal
  from settling at all.

Evidence: `config::tests::user_checks_become_goal_limits` (the exact JSON shape; no `null` field may reach
`create_goal`) and `a_broken_check_is_rejected_when_the_config_loads`; the user-only rule in
`config::tests::user_hooks_and_retention_survive_loading_and_project_ones_are_ignored`;
`v2_driver::configured_checks_gate_the_goal_through_the_config_edge` drives config text → goal limits →
a failing check → repair → `SUCCEEDED`; `cli::the_daemon_carries_configured_checks_into_the_goal` proves the
running daemon stores them on the goal and that `doctor` reports them;
`tui::events_drive_refreshes_and_notes` checks that a round names its checks and a repair names the failing one.

## D-49 The headless `exec` contract is real again (2026-09-25)

While comparing the product surface with the code, the headless entry point turned out to be documented but
partly not implemented. `teamagents exec "prompt"` (the plain form in both READMEs) was rejected as a usage
error because the argument parser demanded `--json`; `exec -` accepted stdin according to the README but
submitted the literal string `-`; `--check COMMAND` was parsed and then silently dropped. Three further
defects came out of the same review: a settlement recorded by an **earlier** run was replayed as the current
run's outcome (the client started at watermark 0), a goal that settled `FAILED`/`BLOCKED` still exited `0`,
and a leader parked by a permanent failure or a click-through approval let `exec` wait for its whole
deadline.

The restored contract is the v1/D-32 one, which is what both READMEs already promised:

- **Prompt**: the positional argument, or everything on stdin when it is `-`; an empty prompt is a usage
  error. Both READMEs document the marker.
- **Exit codes**: `0` settled (goal `SUCCEEDED`, or a direct reply), `1` failed or unfinished (including a
  failed `--check`), `3` an approval is pending (a headless run has nobody to answer it, so it reports
  immediately instead of burning the deadline), `124` the deadline passed, `2` usage or infrastructure.
- **`--check COMMAND`** (repeatable): the user's own acceptance command, run after the turn ends, in order,
  in the isolated shell inside the client's workspace; the first failure stops the list and makes the run
  fail. The verdicts are printed, written to `<state root>/verification.json` and carried in the `--json`
  report. The client reads each command's status from a random marker the wrapper prints, so a command that
  prints its own `(exit 0)` cannot fake a pass. It never runs when the run stopped for an approval.
- **Own outcome only**: the client drains the event log before submitting (a stored `goal_completed` is
  history, not this run's result), reports a permanently failed leader request as the run's failure at once,
  and refuses to submit to a leader whose lifecycle is not `ACTIVE` (parked/paused) instead of queueing work
  nobody drains.
- **Daemon startup**: the daemon's output goes to `<state root>/daemon.log`; when it exits during startup the
  caller reports its own words (and that path) in under a second instead of waiting the full 30-second
  socket window.

**Left open at the time**: `--check` is a *client-side* acceptance command, so it does not become the goal's
runtime `required_checks` (`limits.required_checks`, executed by the driver at the completion boundary,
D-42/§8), and no user surface predefined those. D-50 closes that gap with `[[checks]]` in the user config and
keeps the two clearly distinguished; amending the goal limits of an *already running* session is still not
offered (it would be new protocol surface, and it has no user request behind it).

Evidence: `v2::exec::tests::exit_codes_follow_the_documented_contract`,
`the_check_verdict_reads_the_wrapper_marker`,
`acceptance_commands_run_in_order_and_stop_at_the_first_failure`; `main::tests::exec_takes_the_prompt_from_the_argument_or_from_stdin`,
`exec_refuses_a_missing_or_empty_prompt`; and against a real socket with a scripted leader:
`v2_daemon::headless_runs_report_their_own_outcome_not_an_earlier_settlement`,
`headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code`,
`a_failing_acceptance_command_fails_the_run`, `a_blocked_goal_is_not_reported_as_a_success`,
`a_failed_turn_ends_the_headless_run_instead_of_timing_out`,
`a_parked_approval_ends_the_headless_run_at_once`; and through the real binary:
`cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check`.

## D-48 TUI shortcuts without function keys (2026-09-25)
