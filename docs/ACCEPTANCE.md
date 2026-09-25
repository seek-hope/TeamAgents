# Acceptance matrix (A01–A36)

Baseline: [the design and acceptance baseline](DESIGN.md) §12/§16. Current implementation checked on
**2026-09-25**.

✅ = the listed path has automated evidence (it does not prove every release condition of the scenario);
🔶 = partial coverage or a known gap; ⚠ = not implemented.

`make check` is green (core 91 / engine 136 / tui 29) and `make pty` passes; both are preconditions for every
item below.

## Matrix A01–A36 (per-item evidence)

| Item | Scenario | Evidence |
|---|---|---|
| A01 | A single Leader completes a goal | `v2_driver::end_to_end_shell_then_finish`; three real DeepSeek tasks (2026-09-23) |
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
| A16 | A required check fails | `v2_driver::required_checks_failure_repairs_then_passes`, `required_checks_exhausted_parks_the_goal_blocked` |
| A17 | Artifacts change after a check | `v2_driver::check_inputs_must_still_hold_at_completion` |
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
| A29 | Session isolation and a shared project | `control::begin_request_rejects_instances_of_other_sessions` |
| A30 | Artifact and DB write boundaries | `control::artifact_staging_gc_and_publication_ordering` |
| A31 | Write failure / disk full | `control::disk_full_is_classified_at_the_submit_boundary`, `v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking` |
| A32 | Very large history measurement | `engine/examples/load_probe.rs` plus the local probe report under `review/tmp/` (not part of the tree) |
| A33 | Two daemons / stale lock | `v2_daemon::second_daemon_is_refused_and_shutdown_releases_the_lock` |
| A34 | Incompatible schema | `core::v2::store::open_refuses_unstamped_foreign_and_wrong_version`, `open_migrates_the_previous_schema_version` |
| A35 | Goal deadline | `control::goal_deadline_refuses_new_requests_and_dispatches`, `v2_driver::goal_deadline_parks_the_instance` |
| A36 | Install / init / doctor / cleanup / reopen | `cli::init_prepares_the_v2_root_and_doctor_verifies_it`; the one-off cleanup plus a real re-verification (local probe evidence under `review/tmp/`) |

## Upgrade notes (differences from earlier releases)

- The `sessions/` layout and config of earlier releases (≤ v0.1.2) are **not migrated**: `teamagents init`
  prepares the current state root (default `$XDG_STATE_HOME/teamagents/v2`), `doctor` only reports the old
  directory, and the old sessions, preferences and caches were removed against an explicit inventory
  (already done; see [DECISIONS](DECISIONS.md)).
- The old entry points `--team` / `--resume` / `--plain` / `validate` / `sessions prune` no longer exist: the
  Leader forms the team at runtime and the daemon reconnects by event watermark, with `teamagents exec` as
  the headless entry point. Passing those arguments fails with a clear message instead of being ignored.
