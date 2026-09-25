# Acceptance matrix (A01–A36)

Baseline: [the design and acceptance baseline](DESIGN.md) §12/§16. Current implementation checked on
**2026-09-25**.

✅ = the listed path has automated evidence (it does not prove every release condition of the scenario);
🔶 = partial coverage or a known gap; ⚠ = not implemented.

`make check` is green (core 93 / engine 162 / tui 25 test targets) and `make pty` passes; both are
preconditions for every item below. `make check` includes `make language-check`, which fails on non-English
characters outside the two documented exceptions (`README.zh-CN.md` and the frozen material under
`review/eval`).

## Matrix A01–A36 (per-item evidence)

| Item | Scenario | Evidence |
|---|---|---|
| A01 | A single Leader completes a goal | `v2_driver::end_to_end_shell_then_finish`; three real DeepSeek tasks (2026-09-23); the headless entry is driven end to end by `v2_daemon::headless_runs_report_their_own_outcome_not_an_earlier_settlement` and `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check` |
| A02 | A→B→C→A communication | `control::messages_flow_across_an_authorized_ring` |
| A03 | Limited delegation and parent revocation | `control::grants_narrow_only_and_parent_revocation_cascades` |
| A04 | A queued action meets a revocation | `revocation_blocks_queued_dispatch_until_reauthorized`, `dispatch_rechecks_permission_revision` |
| A05 | Reading another instance's history | `control::read_history_is_user_or_self_only`; the daemon's history surface |
| A06 | A message applied across a restart | `submit_input_applies_context_once_per_envelope`, `command_replay_returns_stored_receipt_and_rejects_conflict` |
| A07 | Permanent start failure | `fail_request_closes_and_parks_without_losing_input`, `v2_spawn_failure::*` |
| A08 | Crash after a tool succeeded, before consumption | `v2_driver::tool_result_is_reused_after_crash_not_reexecuted` |
| A09 | Unknown external outcome | `control::unknown_outcome_parks_running_tasks_and_notifies` |
| A10 | Duplicate dispatch / GO | `jobs_runner::duplicate_go_starts_exactly_one_command` |
| A11 | daemon and runner crash separately | `jobs_runner::daemon_crash_reconnects_the_same_job_without_restart` |
| A12 | A shell service outlives an exit (D-41) | `jobs_runner::a_successful_commands_service_outlives_the_job`; six real `approved_scope` approvals |
| A13 | Cancel / timeout / completion races | `jobs_runner::cancel_*`, `v2_driver::user_cancel_stops_a_running_job` |
| A14 | bubblewrap unavailable | `tools.rs` `IsolationUnavailable`; measured classified failure inside the sandbox (no host fallback) |
| A15 | Environment identity | the runner persists and verifies pid + boot_id + start_ticks (`jobs_runner`) |
| A16 | A required check fails | `v2_driver::required_checks_failure_repairs_then_passes`, `required_checks_exhausted_parks_the_goal_blocked`, and the user-facing path `v2_driver::configured_checks_gate_the_goal_through_the_config_edge` (`[[checks]]` → goal limits → repair → success); `cli::the_daemon_carries_configured_checks_into_the_goal`. **Real model** (runs under `review/tmp/d50-live/`): a configured check really ran at the completion boundary before DeepSeek Flash settled a goal, and neither run that could not satisfy its checks reported success — but for the wrong reason in both (a wire error and a missing `status`, both fixed by D-54), so the *blocking* path with a real model is still only covered by the deterministic tests |
| A17 | Artifacts change after a check | `v2_driver::check_inputs_must_still_hold_at_completion`; a configured check's `inputs` reach the goal unchanged (`config::tests::user_checks_become_goal_limits`) |
| A18 | Multi-instance usage budget | `control::a_worker_shares_the_budget_of_the_goal_its_queue_serves` and related |
| A19 | Truncated stream and connection loss | `providers_fake::truncated_stream_before_output_is_transient`, `providers_stall::*` |
| A20 | Restart after long-context compaction | `control::compression_*`, `v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart` |
| A21 | The user adjusts an instance directly | single-writer `submit_input` context plus the TUI conversation target switch |
| A22 | ALL/ANY wait cycles and timers | `control::blocked_report_flags_dead_waits_not_cycles`, `a_due_timer_closes_the_wait` |
| A23 | A result arrives before the wait is registered | `control::wait_for_an_arrived_result_is_satisfied_at_registration` |
| A24 | A late result after a reset | `control::late_receipt_after_reset_lands_on_the_old_epoch_only` |
| A25 | MCP approval / cancellation / unknown outcome | the six `v2_mcp` tests |
| A26 | Skills permissions | `v2_mcp::skill_call_without_the_binding_fails_honestly` |
| A27 | Heterogeneous providers cooperating | real: DeepSeek and Kimi exchanging messages both ways in one session (local probe evidence under `review/tmp/`, not part of the tree); fake services: `v2_supervisor::heterogeneous_*` |
| A28 | Disconnect, slow client, reconnect | `v2_daemon::handshake_checkpoint_command_and_goal_completion`, `reconnect_backfills_events_after_the_watermark` |
| A29 | Session isolation and a shared project | `control::begin_request_rejects_instances_of_other_sessions`; `cli::cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one` (the session's `--cwd` is what the instances and tools work in, and a client that joins a live session is told the real one) |
| A30 | Artifact and DB write boundaries | `control::artifact_staging_gc_and_publication_ordering` |
| A31 | Write failure / disk full | `control::disk_full_is_classified_at_the_submit_boundary`, `v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking` |
| A32 | Very large history measurement | `engine/examples/load_probe.rs` plus the local probe report under `review/tmp/` (not part of the tree) |
| A33 | Two daemons / stale lock | `v2_daemon::second_daemon_is_refused_and_shutdown_releases_the_lock` |
| A34 | Incompatible schema | `core::v2::store::open_refuses_unstamped_foreign_and_wrong_version`, `open_migrates_the_previous_schema_version` |
| A35 | Goal deadline | `control::goal_deadline_refuses_new_requests_and_dispatches`, `v2_driver::goal_deadline_parks_the_instance` |
| A36 | Install / init / doctor / cleanup / reopen | `cli::init_prepares_the_v2_root_and_doctor_verifies_it`; the one-off cleanup plus a real re-verification (local probe evidence under `review/tmp/`) |

## Headless client contract (`teamagents exec`, D-49)

The headless entry point is a product surface of its own, so its contract is listed separately from the
A-matrix (which describes runtime scenarios):

| Contract | Evidence |
|---|---|
| Plain output, `--json` report, `-` reads the prompt from stdin, an empty prompt is a usage error | `main::tests::exec_takes_the_prompt_from_the_argument_or_from_stdin`, `exec_refuses_a_missing_or_empty_prompt`; `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check` drives the real binary with a pipe and reads the submitted text back out of the leader's context |
| Exit codes `0` settled / `1` failed or unfinished / `3` approval pending / `124` deadline / `2` usage or infrastructure | `v2::exec::tests::exit_codes_follow_the_documented_contract`; against a real daemon: `v2_daemon::a_blocked_goal_is_not_reported_as_a_success` (a `BLOCKED` goal is not exit 0), `a_failed_turn_ends_the_headless_run_instead_of_timing_out`, `a_parked_approval_ends_the_headless_run_at_once` |
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
- The old entry points `--team` / `--resume` / `--plain` / `validate` / `sessions prune` no longer exist: the
  Leader forms the team at runtime and the daemon reconnects by event watermark, with `teamagents exec` as
  the headless entry point. Passing those arguments fails with a clear message instead of being ignored.

## Known gaps (found while auditing the documented surface, 2026-09-25)

- **The project config is not read.** `config::load_user_config_for` merges `<cwd>/.teamagents/config.toml`
  with the documented trust rules (project models are allowed, project tools/skills/instructions need
  `[permissions] trust_project_tools = true`, and hooks/retention/checks may only come from the user config),
  and it has tests, but no entry point calls it: the daemon, TUI and `exec` load the user config only. The
  README and the user guide now state this instead of promising the project file. Wiring it is a decision,
  because it changes what a cloned repository can influence (including a malformed project file failing the
  session start) — it needs the user's call before it lands.
