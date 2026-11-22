# Current acceptance matrix

Baseline: [DESIGN.md](DESIGN.md) §1/§12 and the current decisions in [DECISIONS.md](DECISIONS.md).
The tables below map each requirement to implementation checks and their limits. Run the checks against the source or package under evaluation.

Current source inventory: core 210 / engine 563 / tui 133 test targets.

## Requirements (Q1–Q19) and their evidence

| # | Current requirement | Current evidence and boundary |
|---|---|---|
| Q1 | Task success and long-horizon reliability matter equally at the same model and budget | A01, A10, A16, A35: persistent work, bounded recovery, required checks and usage accounting are implemented; general live-model success rates remain unmeasured |
| Q2 | Language and component boundaries may change | `rust-toolchain.toml`, Rust `core`, `engine` and `tui` crates; test-only Python fake services and probes |
| Q3 | Kernel and runtime define execution semantics | A01, A33: `core/src/kernel`, `core/src/v2`, `engine/src/v2` and the daemon/driver/supervisor path |
| Q4 | Context, messages and tools are logically isolated by default | A03, A05, A26: scoped grants and private-history checks; same-user host execution is not an OS security boundary |
| Q5 | The Leader manages and may delegate attenuated authority | A02, A03, A04: `issue_grant`, parent revocation, task-scoped delegation and dispatch reauthorization |
| Q6 | Observable team TUI inspired by Herdr, backed by the developer CLI | A21, A28, A36: `tui/src/team.rs`, `tui/tests/team.rs`, shared geometry, independent member history and fixed Leader controls; CLI remains available for management |
| Q7 | Closing the UI leaves work running and supports reconnect/intervention | A28: independent daemon, event watermark, `tui/tests/reconnect.rs`, daemon and supervisor tests |
| Q8 | The user talks to the Leader, may read/address members and may pause/cancel | A05, A21: user identity in control, `read_history`, instance intervention and `read_history_is_user_or_self_only` |
| Q9 | Instances are retained and sessions isolated | A24, A29, A33: session registry, lifecycle transitions, reset/fork and workspace policies |
| Q10 | Work continues with optional budgets and bounded failures | A18, A35: work scopes, durable states, budget reservations, recovery and `work_deadline_*` tests |
| Q11 | Required checks must pass; other claims need evidence; review is on demand | A16, A17: required-check repair, completion evidence, `engine/src/v2/reviewer.rs`; live reviewer judgement remains open |
| Q12 | Basic tools, MCP and Skills ship without an external Codex backend | A25, A26: `engine/src/tools.rs`, `engine/src/mcp.rs`, local Cargo/Python/Node workflows; malformed per-edit hash preconditions reject the whole batch unchanged |
| Q13 | Multiple providers and mixed models can cooperate | A27: provider adapters and `v2_supervisor::heterogeneous_instances_run_different_protocols_in_one_session`; broader live matrix remains open |
| Q14 | Workspace is shared by default, with isolated/worktree options | A29: `engine/src/workspace.rs` and `driver::prepare_spawn_workspace`; shared files do not share private conversations |
| Q15 | Authorized work resumes after restart; unknown effects are never blindly replayed | A08, A09, A11, A24: persisted-phase recovery, `OUTCOME_UNKNOWN`, durable operation and runner journals |
| Q16 | Single-instance behavior must not regress and collaboration should show reproducible gain | A01: local scripted collaboration evidence exists; a general matched held-out outcome/cost comparison remains open |
| Q17 | Estimate cost before committing a full multi-round budget | A18, `docs/PRODUCT-COMPARISON.md`: usage is recorded per request/work with known and unknown accounting; a current priced comparison is not claimed |
| Q18 | Old config/session compatibility is not inherently required; migration and cleanup are deliberate | A34, A36: migrations through schema 13 and explicit cleanup are implemented; older writers cannot reopen the migrated store |
| Q19 | Cleanup follows an explicit ownership decision while Git history remains | A36, `tools/README.md`: current tree contains only current source and tools; `tools/` holds executable development and verification utilities |

## Matrix A01–A36 (per-item evidence)

| ID | Scenario | Current evidence and boundary |
|---|---|---|
| A01 | A single Leader completes work, including work performed by the team it builds | `v2_driver::end_to_end_shell_then_finish`, `v2_supervisor::spawned_worker_settles_and_the_leader_completes`, headless execution tests |
| A02 | A→B→C→A communication | `control::messages_flow_across_an_authorized_ring`, `tools/dogfood/team_ring.py` |
| A03 | Limited delegation and parent revocation | `control::grants_narrow_only_and_parent_revocation_cascades`, authority CLI and `V2Authority` |
| A04 | A queued action meets revocation | `control::revocation_blocks_queued_dispatch_until_reauthorized`, dispatch permission-revision checks; malformed-argument regressions also cover refusal before approval and reopening |
| A05 | Reading another instance's history | `control::read_history_is_user_or_self_only`, `V2History`; user may read, another instance may not |
| A06 | A message is applied across a restart | `control::submit_input_keeps_the_envelope_dedup_across_a_reopen`, durable inbox/application checks |
| A07 | Permanent start failure | `v2_spawn_failure`, classified setup failures and parked work rather than false success |
| A08 | Crash after a tool succeeded but before result consumption | Durable operation receipts and `complete_operation_replay_returns_stored_state` |
| A09 | Unknown external outcome | `OUTCOME_UNKNOWN`, `unknown_outcome_parks_running_tasks_and_notifies`, no guessed replay |
| A10 | Duplicate dispatch / duplicate GO | Operation-id state checks and command replay receipts |
| A11 | Daemon and runner crash separately | `V2JobSupervision`, runner journals, supervisor and daemon restart tests |
| A12 | A Shell service outlives command exit | Runner retirement, idle cadence and `a_settled_shell_job_leaves_no_runner_behind` |
| A13 | Cancel / timeout / completion races | Cancel-before-dispatch, escalation, terminal receipt and race tests; cancellation is not stop evidence |
| A14 | Host execution and rejection of retired configuration | Host Shell/MCP and retired-key refusal; host execution is not confinement |
| A15 | Environment and process identity | `engine/src/environment.rs`, `engine/src/jobs`, `boot_id` and recorded process-group observation; escaped/remote effects remain outside |
| A16 | A required check fails | `V2Checks`, configured-check repair, blocked after bounded failed rounds, no false success |
| A17 | Artifacts change after a check | Stale-input completion refusal and `tools/dogfood/stale_check.py`; external service rechecks remain environmental |
| A18 | Multi-instance usage budget | Shared work reservations, known/unknown settlement and `budget_gate_reserves_settles_and_releases` |
| A19 | Truncated stream and connection loss | `V2StreamBoundary`, provider stream limits and transport tests; no invisible-text replay after emission |
| A20 | Context compaction, original-history preservation and restart | `control::compression_*`, long-context restart and source-ID capture tests, `V2Compress`, `V2SearchTree` and ordered-pair coverage |
| A21 | The user adjusts an instance directly | `v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance`, TUI and instance controls |
| A22 | Wait/deadlock diagnosis with ALL/ANY conditions and external paths | `V2Wait`, `engine/src/v2/diagnostics.rs`, blocked-wait reports; not an arbitrary wait-graph proof |
| A23 | Deferred facts, wait registration, sweeps and wakeup races | Durable wait conditions, same-transaction wakeups and `V2Wait`/`V2Retention` |
| A24 | Late receipts after reset/termination or epoch change | `late_receipt_after_reset_lands_on_the_old_epoch_only`, termination normalization and stale-response guards |
| A25 | MCP over stdio and streamable HTTP | `engine/tests/v2_mcp.rs`, `engine/tests/mcp_oauth.rs`, resource list/read and ambiguity rejection; external OAuth matrix remains open |
| A26 | Skills permissions: content cannot mint authority | `V2SkillAuthority`, `skill_read_without_shell`, relative-Skill and revocation tests |
| A27 | Heterogeneous providers and mixed models in one team | Provider adapter/fake-service and heterogeneous-supervisor tests; real-provider cooperation is not established by fakes |
| A28 | Disconnect, slow client, reconnect and watermark recovery | `V2Daemon`, bounded protocol pages, persistent history-refresh retry and explicit oversized-event resync; wider client/network coverage remains open |
| A29 | Session isolation with shared project/workspace options | Workspace policies, retirement guards and `V2Workspace`; file sharing does not imply private-history sharing |
| A30 | Artifact persistence, publication, release and garbage collection | `V2Artifact`, `V2ExportSnapshot`, staging/live/deleting states and export tests |
| A31 | Disk full / persistence-write failure | `disk_full_is_classified_at_the_submit_boundary`, storage-backpressure supervision and no new side effects after latch |
| A32 | Large-history, storage, rendering and slow-reader behavior | Lazy input revisions, bounded idle reads, viewport allocation checks and load-probe entry; latest full performance figures are not claimed |
| A33 | Second daemon, coordinator exclusion and lock recovery | `second_daemon_is_refused_and_shutdown_releases_the_lock`, `V2Coordinator`, D-522 listener/uncertainty guards and explicit-stop refusal checks |
| A34 | Foreign or wrong-version state-store refusal | `open_refuses_unstamped_foreign_and_wrong_version`, `open_migrates_v8_to_the_session_search_tree`, conflict rollback and `V2Store` |
| A35 | Work deadline | `work_deadline_refuses_new_requests_and_dispatches`, deadline park and cancel/release behavior |
| A36 | Store/session lifecycle, search/export and guarded cleanup | `engine/tests/session_browser.rs`, `engine/tests/session_workspace.rs`, `session_export_tests.rs`, session CLI, `tools/install_check.py`, current schema 13 and maintained development tools |

## Workflow checks

The project config is read under explicit user trust. `tools/dogfood/project_config.py` exercises trusted and untrusted repositories. The user config owns the default model, permissions, hooks, checks, limits, retention and update policy.

| Behavior | Runnable check |
|---|---|
| Rewind a Leader input and preview tracked file changes | `engine/tests/cases/rewind.rs` covers dirty working bytes, stale preview refusal, restart and replay |
| Hold input until work settles, preserve its checks and attachments | `core/tests/input_queue.rs` and `core/src/v2/control/work_tests.rs` |
| Bind a Worker result to the assigned task and release a temporary Worker at a quiet boundary | `engine/tests/v2_supervisor.rs` and `engine/tests/v2_driver.rs` |
| Resolve session models, preserve retries and exercise heterogeneous providers | `core/tests/role_models.rs`, `engine/tests/v2_driver.rs` and `engine/tests/v2_supervisor.rs` |
| Restore a workspace and retain drafts across session switches | `engine/tests/session_workspace.rs` and `tools/dogfood/tui_sessions.py` |
| Refresh the catalogue and manage a local router | `engine/tests/cases/catalog.rs` and `engine/tests/cases/local_router.rs` use loopback services |
| Check CLI and TUI behavior in isolation | `engine/tests/interface.rs`, `tui/tests/interface.rs` and `make pty` |
| Build and install the release archive | `make release-rehearsal` exercises an isolated install and startup |

## Current probe inventory

Offline probes are `boundary.py`, `budget.py`, `truncation.py`, `max_retries.py`, `project_config.py`,
`sessions_prune.py`, `instructions.py`, `input_latency.py`, `tui_panels.py`, `tui_sessions.py`, `geometry.py`,
`tui_reconnect.py`, `providers.py --self-check`, `shutdown.py`, `daemon_stop.py`, `stream_json.py`, `runners.py`
and `artifacts.py`.

Model-dependent probes are `crash.py`, `unknown_outcome.py`, `cancel.py`, `lifecycle_run.py`, `deadline.py`,
`job_identity.py`, `checks.py`, `stale_check.py`, `two_gates.py`, `exec_check.py`, `queued_input.py`,
`runtime_note.py`, `hooks.py`, `skills.py`, `web.py`, `mcp.py`, `mcp_http.py`, `workspace.py`, `authority.py`,
`tui.py`, `team_ring.py`, `providers.py` and `protocols.py`. They require separately bounded credentials and do
not form a statistical acceptance result. `ssh_pty.py` is an explicit isolated-loopback environment probe.

## Current open obligations

- Matched held-out single-instance/team/alternative-harness comparisons must measure artifact correctness,
  completed work, false-success claims, interventions, usage and priced cost separately. Unknown usage/prices
  are not zero. The prioritized queue is [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md).
- Independent multi-model Reviewer/adversarial evaluation, semantic tree/summary completeness, navigation
  usability, reduced user lostness and general long-horizon success remain empirical obligations.
- Current real-provider protocol/cooperation/image coverage, external OAuth, public SSH/graphical terminals,
  clean-host install/update/rollback, schema transitions, physical hosts and native ARM/packages need separate
  acceptance. Scripted services and user emulation do not establish these results.
- Live cross-provider switching, provider-side fetch DNS/redirect/rebinding,
  broader proxy configurations, session startup/filesystem races and full-backlog connection deadlines remain open.
- Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition, arbitrary-string
  capability algebra and Policy B source-bound bidirectional fair-trace equivalence remain unestablished.
- Scheduler/provider fairness, escaped descendants, remote effects and real power-loss timing remain
  environmental obligations. Cancellation and process absence do not reconstruct unknown outcomes.
- MCP revision `2025-06-18` has local executable coverage; external OAuth/SDK/client/extension matrices,
  resumable streams and broader versioned integration contracts remain open.
- Artifact references remain protected after owner release; no destructive reference-expiry policy is selected.
  Artifact lifetime under future retention policies and broader dependency ecosystems remain open.
