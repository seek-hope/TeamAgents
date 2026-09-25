# Acceptance matrix (A01–A36)

Baseline: [the design and acceptance baseline](DESIGN.md) §12/§16. Current implementation checked on
**2026-09-25**.

✅ = the listed path has automated evidence (it does not prove every release condition of the scenario);
🔶 = partial coverage or a known gap; ⚠ = not implemented.

`make check` is green (core 98 / engine 196 / tui 31 test targets) and `make pty` passes; both are
preconditions for every item below. `make check` includes `make language-check`, which fails on non-English
characters outside the two documented exceptions (`README.zh-CN.md` and the frozen material under
`review/eval`).

## Matrix A01–A36 (per-item evidence)

| Item | Scenario | Evidence |
|---|---|---|
| A01 | A single Leader completes a goal (and the team it builds runs) | `v2_driver::end_to_end_shell_then_finish`; three real DeepSeek tasks (2026-09-23); a real delegation run: 3 instances, both delegated tasks `SUCCEEDED`, both `[[checks]]` commands passing, 16.3 s (`review/tmp/dogfood/`); the headless entry is driven end to end by `v2_daemon::headless_runs_report_their_own_outcome_not_an_earlier_settlement` and `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check` The web tools are verified live (`review/dogfood/web.py`, 2026-09-26, both providers): a bound `web_fetch` puts a real page's body into the conversation, and the same tool refuses a private address (`refusing private address for 127.0.0.1`) — the SSRF guard, observed rather than only unit-tested. `doctor` now also says when a config declares no web binding at all, since the model then gets neither tool (D-79). The private-address guard behind that tool is cross-checked against its reference implementation (Python's `ipaddress`): every block edge of its tables plus 1000 random addresses agreed (1224 cases), and the boundary cases are a committed test (`tools::tests::the_address_guard_matches_the_reference_at_every_block_edge`, D-80). |
| A02 | A→B→C→A communication | `control::messages_flow_across_an_authorized_ring`; the session grants the Leader `message`@session at bootstrap, so `send` is offered and authorized without extra setup (`cli::the_daemon_grants_the_leader_the_team_authority`, `v2_driver::the_leader_is_authorized_to_build_the_team_by_default`); the offered surface never promises what the instance cannot dispatch (`V2Grants::OfferedToolsAreAuthorized`, correspondence `v2_supervisor::the_offered_surface_follows_the_grants`); the user can grant and revoke those connections through `teamagents authority` (D-61, see A03) |
| A03 | Limited delegation and parent revocation | `control::grants_narrow_only_and_parent_revocation_cascades`; the Leader's default authority is exactly these session-scoped grants, and revocation still removes the tool and fails the call closed. **The user can exercise it** (D-61): `cli::the_authority_surface_grants_and_revokes_through_the_daemon` drives the real binary against a real daemon — the list carries the ids, a grant to a spawned worker makes the dispatch question `holds_covering_grant(worker, "shell", "workspace")` true, a grant derived with `--parent` dies with its parent (cascade of exactly 2), the question is false again afterwards, and a pair no check asks about is refused with a reason instead of written (`core/src/v2/capability.rs`'s table test pins that pair table); `v2_supervisor::a_users_grant_reaches_the_workers_surface_at_the_next_request` shows the granted tool appears on the worker's next request and leaves it again after the revocation; formally, `V2Authority` (`make verify-model-all`) proves the view carries what a revoke needs, the cascade takes exactly the subtree, and its three negative controls (`make verify-model-counterexamples`) are refuted. **Real model** (`review/dogfood/authority.py`, 2026-09-25, isolated state root `/tmp/ta-authority-run`): a worker reported it could not run shell commands, the grant (revision 8) was issued, the same worker then ran the command (turn 2, 14.0 s, exit code 0, `proof.txt` present) and the revocation (revision 11) left no live shell grant — 13 model requests, both tasks `SUCCEEDED`, no failed request The two questions left open are in `docs/DECISIONS.md` D-61 ("Left open") |
| A04 | A queued action meets a revocation | `revocation_blocks_queued_dispatch_until_reauthorized`, `dispatch_rechecks_permission_revision` |
| A05 | Reading another instance's history | `control::read_history_is_user_or_self_only`; the daemon's history surface |
| A06 | A message applied across a restart | `submit_input_applies_context_once_per_envelope`, `command_replay_returns_stored_receipt_and_rejects_conflict`; an input that arrives during a turn is queued and applied at the next boundary instead of being stored behind that turn's reply (D-63): `an_input_inside_a_turn_waits_for_the_boundary`, `v2_supervisor::an_input_arriving_during_a_turn_enters_at_the_next_boundary`, and formally `V2Control::InputLandsAtTheBoundary` / `QueuedInputEntersTheContext` with the refuted control `MC_control_midturninput.cfg`. The queueing is what makes a client's outcome attribution a question at all, and D-72 answers it: a run reports its own input's outcome (`SettlementFollowsATurnAfterTheLanding`, refuted by `MC_control_landing.cfg`; the headless contract's own row is above) |
| A07 | Permanent start failure | `fail_request_closes_and_parks_without_losing_input`, `v2_spawn_failure::*`; **no turn storm** also covers the *model* stopping rather than failing (D-65): a plain reply opens no further turn (`v2_supervisor::a_prose_reply_leaves_one_turn_and_the_delegator_resolves_the_task`, which fails with the pre-fix rule), the task stays `RUNNING` for the delegator or the user, and formally `V2Control::NoTurnWithoutWork` refutes the counterfactual `MC_control_reask.cfg`. The same invariant covers the other direction (D-71): the runtime's own closing word is a committed tail, so a settlement is never answered with a fresh turn — the refuted control `MC_control_runtimeTail.cfg` shows the violating state (`tail = "runtime"`, `phase = "MODEL_PENDING"`). And a turn the runtime closed with nothing settled is reported as such, never as a reply (the runtime's own sentence) or a timeout: `v2_daemon::a_turn_closed_by_the_runtime_without_a_settlement_is_not_a_reply` |
| A08 | Crash after a tool succeeded, before consumption | `v2_driver::tool_result_is_reused_after_crash_not_reexecuted` **Live** (`review/dogfood/crash.py`, 2026-09-26, both providers): the daemon is killed while the instance is in `TOOLS_PENDING` with the shell command running; the runner (a separate process) finishes it, the restarted session consumes the receipt instead of re-running the command (`runs.log` holds exactly **one** line across the crash) and settles the goal `SUCCEEDED` |
| A09 | Unknown external outcome | `control::unknown_outcome_parks_running_tasks_and_notifies` |
| A10 | Duplicate dispatch / GO | `jobs_runner::duplicate_go_starts_exactly_one_command` |
| A11 | daemon and runner crash separately | `jobs_runner::daemon_crash_reconnects_the_same_job_without_restart` — and the same live probe is exactly this shape with a model in the loop: the daemon dies, the *runner* keeps the job and the next daemon picks the receipt up |
| A12 | A shell service outlives an exit (D-41) | `jobs_runner::a_successful_commands_service_outlives_the_job`; six real `approved_scope` approvals |
| A13 | Cancel / timeout / completion races | `jobs_runner::cancel_*`, `v2_driver::user_cancel_stops_a_running_job` |
| A14 | bubblewrap unavailable | `tools.rs` `IsolationUnavailable`; measured classified failure inside the sandbox (no host fallback) |
| A15 | Environment identity | the runner persists and verifies pid + boot_id + start_ticks (`jobs_runner`) |
| A16 | A required check fails | `v2_driver::required_checks_failure_repairs_then_passes`, `required_checks_exhausted_parks_the_goal_blocked`, `v2_driver::configured_checks_gate_the_goal_through_the_config_edge` (`[[checks]]` → goal limits → repair → success); `cli::the_daemon_carries_configured_checks_into_the_goal`. **Real model, re-runnable on both protocols**: `python3 review/dogfood/checks.py [--provider deepseek\|kimi]` — a configured check that can never pass, a task the model completes, and the whole gate exercised end to end: round 1 → repair → round 2 → round 3 → goal **BLOCKED** with the ledger naming `check_id: impossible` / `class: exit`, `exec` exiting **1** and no success reported — measured 2026-09-25 on **deepseek** (11 model requests, 9.7 s) and on **kimi** (the `responses` protocol, 8 requests, 37.8 s), the artifact exact in both, the model refusing to bypass the gate. This harness found and closed two defects: D-70 (the check round's synthetic assistant call needed `reasoning_content` on DeepSeek's thinking wire, or the repair turn itself died with HTTP 400) and D-71 (the Kimi run reported the runtime's own block note as the *reply* with exit 0 — a false success; the block was not even an event). The headless half is pinned by `v2_daemon::a_runtime_blocked_goal_is_not_reported_as_a_reply` (exit 1, `goal_status: BLOCKED`, `reply: null`, the settlement event carrying `blocked_by: runtime`, and the instance's tail entry of kind `runtime`) |
| A17 | Artifacts change after a check | `v2_driver::check_inputs_must_still_hold_at_completion`; a configured check's `inputs` reach the goal unchanged (`config::tests::user_checks_become_goal_limits`) |
| A18 | Multi-instance usage budget | `control::a_worker_shares_the_budget_of_the_goal_its_queue_serves` and related; the ceiling is *reachable* by the user (D-64): `config::user_limits_bound_every_goal`, `cli::configured_limits_reach_the_goal_and_really_bound_the_session` (the goal carries `max_total_tokens`, an unset ceiling stays `{}`) and `cli::a_tiny_configured_ceiling_parks_the_session_instead_of_running_it` (a 4-token ceiling parks the leader with the budget as the reason); the TUI renders the limits it is given (`checkpoint_defaults_to_the_leader_and_tracks_budget`: `usage 10/1000 · ends in 30m`); formally `V2Control::ReservationsAdmitted` / `AdmissionGate` |
| A19 | Truncated stream and connection loss | `providers_fake::truncated_stream_before_output_is_transient`, `providers_stall::*` |
| A20 | Restart after long-context compaction | `control::compression_*`, `v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart` |
| A21 | The user adjusts an instance directly | single-writer `submit_input` context plus the TUI conversation target switch; a message sent while the instance is mid-turn is queued with a visible note (`tui::a_queued_input_is_visible_in_the_conversation`, `exec`'s `input_queued` report) and never silently dropped (D-63); what such an input's *run* reports is its own turn's outcome, and an input a reset sealed is reported `undelivered` instead of timing out (D-72). The interventions are reachable headlessly too (D-68): `v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance` drives the real binary through the D-65 flow (task `RUNNING` with an idle assignee, pause/resume, `terminate` refused without `--yes`, cancel → the delegator wakes and the goal settles) |
| A22 | ALL/ANY wait cycles and timers | `control::blocked_report_flags_dead_waits_not_cycles`, `a_due_timer_closes_the_wait` |
| A23 | A result arrives before the wait is registered | `control::wait_for_an_arrived_result_is_satisfied_at_registration` |
| A24 | A late result after a reset | `control::late_receipt_after_reset_lands_on_the_old_epoch_only` |
| A25 | MCP approval / cancellation / unknown outcome | the six `v2_mcp` tests; the approval *decision* is reachable headlessly (D-67): `v2_daemon::the_approvals_cli_lists_and_decides_a_parked_operation` drives the real `teamagents approvals` binary against a real daemon (list, prefix decision, typo refused, the goal completes after the decision), and the same flow with a real model is recorded in D-67. Formally the gate is `V2Control::NoEffectBeforeApproval` ("an operation needing approval has no effect before it is approved") And the service a user *configures* is really bound (D-74): `[tools.<name>] kind = "mcp"` in the user config (or a trusted project config) loads at session start — before D-74 no surface could bind one at all, so the model never saw the service's tools: `bound::a_declared_mcp_service_is_bound_without_naming_it_in_the_bindings` (fails pre-fix: `left: []`), `v2_supervisor::a_configured_mcp_service_reaches_the_members_surface` (the leader's offered surface carries `probe_ping`), `cli::doctor_probes_isolation_and_config_errors` (a row per declared service, with a WARN for a command that cannot run) and the real-model harness `python3 review/dogfood/mcp.py [--provider kimi]` (the server's log shows initialize/tools/list/tools/call and the run reports the token the server generated — deepseek 2.5 s, kimi 9.6 s). |
| A26 | Skills permissions | `v2_mcp::skill_call_without_the_binding_fails_honestly`; the registry is *visible* (D-66): `cli::doctor_reports_the_skills_registry_and_missing_configured_paths` reports how many skills a configured root yields and warns, naming the path, when a configured root or instruction file is missing (a clean first run shows the shipped `~/.agents/skills` as one WARN instead of a silently empty skill list). **Real model** (`review/dogfood/skills.py`, 2026-09-25/26): a skills root with one skill whose *body* carries a token generated for the run (deliberately absent from the YAML description, so searching alone cannot reveal it) — the model called `skill {action: read, name: canary}`, the receipt carried the body with the token, it then followed the instructions (the file with the token exists), and `doctor` reported the same registry (`1 skill(s) under 1 configured root(s)`); deepseek 5.9 s, kimi 17.4 s, both goals `SUCCEEDED`. The permission half stays with the deterministic checks named above: a member's offered surface follows its grants, and a skill call without the binding fails honestly The *search* half is verified live as well (2026-09-26): a second skill whose keyword (`frobnication`) appears only in its description is found by `skill {action: search}` — the action sequence `read canary → search frobnication → read inbox-triage` is in the conversation, the hit line carries name + description, and the searched-for skill's instructions are followed; deepseek and kimi both, doctor reporting `2 skill(s) under 1 configured root(s)`. |
| A27 | Heterogeneous providers cooperating | **re-runnable real-model harness**: `python3 review/dogfood/providers.py` — one session, the Leader on DeepSeek Flash (native window) and a worker spawned on the user's Kimi entry, delegating a file write and waiting for it; measured 2026-09-25: 7 model requests, 14.0 s, goal `SUCCEEDED`, members `deepseek-flash` / `k3-256k`, the task `SUCCEEDED` and the artifact exact. Each member's model is recorded and visible (D-69: `v2_daemon::the_snapshot_reports_each_members_model`, the TUI panel test, `teamagents instances`). Fake services: `v2_supervisor::heterogeneous_*` |
| A28 | Disconnect, slow client, reconnect | `v2_daemon::handshake_checkpoint_command_and_goal_completion`, `reconnect_backfills_events_after_the_watermark` |
| A29 | Session isolation and a shared project | `control::begin_request_rejects_instances_of_other_sessions`; `cli::cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one` (the session's `--cwd` is what the instances and tools work in, and a client that joins a live session is told the real one) |
| A30 | Artifact and DB write boundaries | `control::artifact_staging_gc_and_publication_ordering` |
| A31 | Write failure / disk full | `control::disk_full_is_classified_at_the_submit_boundary`, `v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking` |
| A32 | Very large history measurement | `cargo run --offline --manifest-path engine/Cargo.toml --example load_probe -- review/tmp/<fresh dir> --steps 250 --payload 20000` (in-tree; raw `report.json` stays in the ignored evidence directory). Measured 2026-09-25: a ~1.26M-token synthetic history built through the production `Control` entry — append p50 6.1 ms / p95 9.5 ms, turn step p50 11.1 ms, a 501-message request built in p50 279 ms, daemon history page (200 entries) p50 23.8 ms, multi-instance reads 1/4/16 readers p50 22.5/87.3/505.8 ms, peak RSS 102 MiB, 10.7 MB of database growth (42.6 KB per step), reopen verification 1.7 ms, WAL + `synchronous=FULL` (synthetic text is not a tokenizer input: the numbers describe local overhead, never model capability) |
| A33 | Two daemons / stale lock | `v2_daemon::second_daemon_is_refused_and_shutdown_releases_the_lock` |
| A34 | Incompatible schema | `core::v2::store::open_refuses_unstamped_foreign_and_wrong_version`, `open_migrates_the_previous_schema_version` (a `1`-stamped store still reaches the current version: the chain walks one step at a time in one transaction), `migrate_rewrites_the_runtimes_closing_notes` (D-71: schema 2 → 3 moves the runtime's closing notes out of the member's voice, leaving the member's own answers and the tool receipts untouched); probed on a real pre-fix state root (`/tmp/ta-providers-run`, written by the previous build) — `schema_version` 2 → 3 and `i-leader:0:13` `assistant`/`role: assistant` → `runtime`/`role: user` on one daemon boot |
| A35 | Goal deadline | `control::goal_deadline_refuses_new_requests_and_dispatches`, `v2_driver::goal_deadline_parks_the_instance`; the deadline is *reachable* by the user (D-64): `config::user_limits_bound_every_goal` and `cli::configured_limits_reach_the_goal_and_really_bound_the_session` (the goal's `deadline` is ~15 minutes out for `deadline_minutes = 15`, and the duration key is not stored on the goal); formally `V2Control::NoRequestAfterDeadline` with the refuted control `MC_control_deadline.cfg` |
| A36 | Install / init / doctor / cleanup / reopen | `cli::init_prepares_the_v2_root_and_doctor_verifies_it`; the one-off cleanup plus a real re-verification (local probe evidence under `review/tmp/`). The **published install path** is verified live (`python3 review/install_check.py`, 2026-09-26): the release archive downloads, its SHA-256 matches the published manifest, `install.sh --archive … --bin-dir …` installs both binaries and they run (`version` exit 0, a usage line, the TUI refusing a session-less start); and the failure mode is verified with a corrupted archive, which the installer refuses with `SHA-256 verification failed; installed binaries were left untouched` (nothing written into the bin directory) — the artifact's *vintage* is a separate, open gap below |

## Headless client contract (`teamagents exec`, D-49)

The headless entry point is a product surface of its own, so its contract is listed separately from the
A-matrix (which describes runtime scenarios):

| Contract | Evidence |
|---|---|
| Plain output, `--json` report, `-` reads the prompt from stdin, an empty prompt is a usage error | `main::tests::exec_takes_the_prompt_from_the_argument_or_from_stdin`, `exec_refuses_a_missing_or_empty_prompt`; `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check` drives the real binary with a pipe and reads the submitted text back out of the leader's context |
| Exit codes `0` settled / `1` failed or unfinished / `3` approval pending / `124` deadline / `2` usage or infrastructure | `v2::exec::tests::exit_codes_follow_the_documented_contract`; against a real daemon: `v2_daemon::a_blocked_goal_is_not_reported_as_a_success` (a `BLOCKED` goal is not exit 0), `a_failed_turn_ends_the_headless_run_instead_of_timing_out`, `a_parked_approval_ends_the_headless_run_at_once` |
| A turn the runtime closed with nothing settled is `unsettled` (exit 1) — never a reply made of the runtime's own words, never a timeout | `v2_daemon::a_turn_closed_by_the_runtime_without_a_settlement_is_not_a_reply` (reported in <30 s), `a_runtime_blocked_goal_is_not_reported_as_a_reply`; `tui::the_runtimes_closing_note_is_not_the_members_message` (the same entries are never rendered as the member's message) |
| A queued input's run reports **its own** outcome (D-72) — never a settlement or a reply the turn it waited behind produced | `v2_daemon::a_queued_input_is_not_answered_by_the_previous_turns_settlement` (with the pre-fix decision restored the same test fails: `left: Completed, right: Reply`), `a_queued_input_is_not_answered_by_the_previous_turns_reply`, `v2::exec::tests::an_outcome_before_the_runs_own_input_is_not_its_outcome`; real model, both protocols: `python3 review/dogfood/queued_input.py [--provider kimi]` (run 1 settles `SUCCEEDED`, the queued run reports `end=reply`, `goal_status: null` and its own word — the pre-fix build reported `end=completed / goal=SUCCEEDED / reply=null` for it); formally `V2Control::SettlementFollowsATurnAfterTheLanding`, refuted by `MC_control_landing.cfg` |
| An input the runtime could never deliver is `undelivered` (exit 1) — not a timeout after the caller's deadline | `v2_daemon::a_queued_input_a_reset_sealed_is_reported_undelivered` (a reset closes the epoch while the input waits; the runtime names what it seals — `envelopes_sealed`) |
| The runtime's closing note stays in the conversation and a later turn still rides the wire | `core::v2::control::closing_a_turn_answers_its_finish_call` (the note is a `runtime` entry in the user's voice, next to the answered `finish`); real model on both protocols: `python3 review/dogfood/runtime_note.py --providers deepseek,kimi` (2026-09-25: turn 1 settles `SUCCEEDED`, turn 2 is an ordinary `reply` with exit 0 — the thinking-mode chat wire and Kimi's `responses` wire both accept the following request) |
| A settlement recorded by an earlier run is not this run's outcome | `v2_daemon::headless_runs_report_their_own_outcome_not_an_earlier_settlement` (second run after a settled goal must report the reply, not `SUCCEEDED`) |
| `--check COMMAND` runs after the turn in the isolated shell in the client's workspace, stops at the first failure, gates the exit code and writes `<state root>/verification.json` | `v2::exec::tests::acceptance_commands_run_in_order_and_stop_at_the_first_failure`, `the_check_verdict_reads_the_wrapper_marker`; `v2_daemon::headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code`, `a_failing_acceptance_command_fails_the_run`; `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check` (the real binary writes the ledger) |
| A parked or paused leader refuses new input (`2`) instead of queueing it | `v2_daemon::a_failed_turn_ends_the_headless_run_instead_of_timing_out` (the failed turn parks the leader; the next run refuses with "nothing was submitted") |

`--check` is a **client-side** acceptance command: it decides `exec`'s exit code after the turn. The goal's
runtime `required_checks` are the stronger contract and come from the user config's `[[checks]]` (D-50,
`docs/USER-GUIDE.md` §2.1); a project file may not define them, and a running session's goal limits cannot be
amended (D-49/D-50).

## Upgrade notes (differences from earlier releases)

- The `sessions/` layout and config of earlier releases (≤ v0.1.2) are **not migrated**: `teamagents init`
  prepares the current state root (default `$XDG_STATE_HOME/teamagents/v2`), `doctor` only reports the old
  directory, and the old sessions, preferences and caches were removed against an explicit inventory
  (already done; see [DECISIONS](DECISIONS.md)).
- The session's permission mode can also come from the user config (D-75): `[permissions] mode = "full_auto"`
  makes host execution the default for the sessions you start, `--full-auto` still asks for it for one boot,
  and a project file can never set it (only the user config is read for the mode).
- The old entry points `--team` / `--resume` / `--plain` / `validate` / `sessions prune` no longer exist: the
  Leader forms the team at runtime and the daemon reconnects by event watermark, with `teamagents exec` as
  the headless entry point. Passing those arguments fails with a clear message instead of being ignored — and
  since D-73 that rule holds for *every* unserved argument, not only the removed ones: a bare word
  (`teamagents hello`, a typo'd verb, a pasted prompt) is a usage error naming the word (it used to boot a
  session and drop it), `-v`/`--verbose` is refused with a pointer (it was accepted and never honoured), and
  the front-end refuses the flags the daemon owns (`--cwd`/`--full-auto`) with the pointer to the engine's own
  (`cli::a_bare_word_and_verbose_are_refused_without_starting_a_session`,
  `tui::cli_flags::the_tui_refuses_the_flags_the_daemon_owns`; the first also asserts that no daemon socket was
  created, so a refused argument has no side effect).

## Known gaps (found while auditing the documented surface, 2026-09-25)

- **The published release is the earlier implementation, and shares the tree's version.** `review/install_check.py` verifies the
  documented install path end to end (mechanics and the refusal above), and the published `v0.1.2` artifact it installs is the
  *pre-v2* product: its `--help` is not in English and still offers `validate`, `sessions prune`, `--plain`, `--resume` and
  `--team`, none of which the documented surface has (D-52/D-73 removed them), so "install the latest release" does not install
  what the README and `docs/USER-GUIDE.md` describe. `engine/Cargo.toml` (and `core`/`tui`) still say `0.1.2`, the version the
  existing tag already names, and the release workflow refuses a tag that does not equal `v<version>` — so a new release needs a
  version bump first. The machinery itself is sound and re-runnable (`.github/workflows/release.yml` builds musl-static binaries,
  packages the docs and `install.sh`, SHA256SUMS them, smoke-installs the exact archive and publishes the assets); cutting the
  release is the user's decision. Until then `docs/INSTALL.md` says the install docs describe the tree, not the artifact.

- **A worktree member's branch has no merge surface.** The `git_worktree` policy (§12.3/D-46) gives a
  member its own branch and checkout, retirement refuses to delete an unmerged one, and the real-model
  harness `review/dogfood/workspace.py` walks the whole lifecycle (D-76) — but nothing merges the
  branch: `workspace::merge_branch` and `workspace::member_worktrees` have no caller anywhere in the
  tree, and the name lives only in `<state root>/instances/<id>/worktree.json` (or `git worktree
  list`). Today the user merges with git, or a Leader with `shell@workspace` does; a
  `teamagents instances merge --id` verb (or a Leader-side merge tool) is new surface and needs the
  user's word first.

- **`[retention]` is accepted but nothing is archived or pruned.** DESIGN §9 promises ordinary history is
  "archived or cleaned per user configuration" while live references and evaluation evidence are never
  evicted; `Retention.archived_days`/`history_days` are parsed, kept user-config-only (like hooks and
  checks) and read by nothing — `doctor` used to print them as `[ok ]`, and since D-75 it prints a WARN
  saying they are not applied and nothing is deleted. Implementing it means deleting data under
  conditions that need their own verification (never evict a live reference or evaluation evidence),
  so it needs the user's word before it lands.

- **A settled goal has no product surface to open a new one** (found while auditing the authority surface,
  2026-09-25; verified by `engine/tests/v2_supervisor.rs::a_settled_goal_leaves_a_later_delegation_without_an_active_goal`):
  `delegate_task` requires an ACTIVE goal, the kernel offers no create-goal tool, and the runtime creates no
  goal when a later user input arrives — so the *second* instruction of a session cannot build a team, and the
  model is told to "create a new goal (create_goal) before delegating" without a way to do it (the command
  exists in the protocol; only a hand-written client can send it). The test pins the current behaviour: the
  second input leaves exactly one goal, `SUCCEEDED`, the delegation receipt names the closed goal, and the
  worker never runs. Whether the runtime should open a goal per user input, or the Leader should be given a
  tool to open one, is a design decision (D-42/D-56 touch it) that needs the user's word.
- **A model that stops settling its task leaves a visible wait, and the runtime does not resolve it** (the
  remaining ceiling of D-65, measured again 2026-09-25): a plain reply ends the instance's turn (that is the
  fix — the pre-fix clause asked **169 model requests / 1,226,717 prompt tokens / 181 context entries in ~15
  minutes** on a real DeepSeek Flash session, `/tmp/ta-authority-probe3`), so the delegated task stays
  `RUNNING` and the delegator's `wait` stays pending. The levers are the user's: cancel the task (`c` in the
  tasks panel, `teamagents tasks cancel`, which satisfies the wait — a `BLOCKED` task would not) or re-dispatch
  the work as a new task. Whether the *runtime* should also offer a bounded "no progress" path (cancel or park
  the task after N unsettled turns) is a design decision that needs the user's word and its own verification.
  Measured consequence, from `review/dogfood/providers.py` (2026-09-25): when the *worker* answers with prose
  instead of calling `finish`, its task never settles, and a leader that verifies the artifact itself can
  settle the goal `SUCCEEDED` with that task still open — `complete_goal` checks open **operations**, not open
  **tasks** (that run: 17 requests, 252 s, `answer.txt` exact, no `task_completed` event; a re-run on the same
  build had the worker call `finish` and passed in 23.1 s, 8 requests — the harness is intermittent because the
  model is). This is not an implementation slip: §4.2's word for the completion transaction is "the acceptance
  references, the open-operation check and the goal/task outcome", and the delegator's own `wait` is the
  design's mechanism for holding a leader until its delegated work resolves (here the leader used a timer wait
  and verified the artifact itself, which is a defensible leader's call, not a runtime one). Whether the
  runtime should *additionally* refuse a settlement while the goal's delegated tasks are open is therefore a
  design question about team semantics, not a bug — it needs the user's word. Related and also open: whether
  the runtime should *interrupt* a running turn when the user sends something, instead of holding the input to
  the boundary as D-63 now does.
- **A worker needs the user's grant for the shared-workspace shell** (D-61): a spawned worker holds no
  `shell@workspace` (§5.1), so until the user runs `teamagents authority grant --subject <id> --action shell
  --scope workspace` it works with the file, web and skill tools only. The surface exists and is verified, but
  whether the *default* should be different — workers getting the shell, or the Leader being allowed to hand
  out the authority it holds — is a design decision that changes §5.1's spawn contract and needs the user's
  call; both options are recorded in `docs/DECISIONS.md` D-61 ("Left open").
- **The project config is not read.** `config::load_user_config_for` merges `<cwd>/.teamagents/config.toml`
  with the documented trust rules (project models are allowed, project tools/skills/instructions need
  `[permissions] trust_project_tools = true`, and hooks/retention/checks may only come from the user config),
  and it has tests, but no entry point calls it: the daemon, TUI and `exec` load the user config only. The
  README and the user guide now state this instead of promising the project file. One consequence is already
  closed: because that loader is the only caller of the path validator, a bad `skills_paths` /
  `instruction_files` entry in the *user* config used to be silent — `doctor` now reports what they resolve to
  and warns when a configured path is missing (D-66). Wiring it is a decision,
  because it changes what a cloned repository can influence (including a malformed project file failing the
  session start) — it needs the user's call before it lands.
