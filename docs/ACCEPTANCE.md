# Current acceptance matrix

Baseline: [DESIGN.md](DESIGN.md) §1/§12 and the current decisions in [DECISIONS.md](DECISIONS.md).
This file records current behavior and reproducible evidence, not historical acceptance runs.

✅ means the named local path has automated evidence. 🔶 means implementation exists but the stated external,
empirical or full-system obligation remains open. No icon is a proof of every release condition.

Current source inventory: core 198 / engine 536 / tui 121 test targets; the full gate passed.
Installed CLI and TUI source: `cab1c47cda05d49b63b55075828be4ffe6402077`, merging the accepted D-528 candidate
`ec1f1acbc6bfd151603f9cb04ebccc14696e8909`. Both x86_64 Linux musl binaries were rebuilt and atomically installed;
current installation probes and artifact hashes are recorded below.
The last complete package baseline is `d96d7c8acf54b951030140f076540cbb730f88ab`. Delivery CI at
`6767b32c4a35fb71c476ca7fb81858d127ba4905` passed `test`, `test-aarch64`, `formal-tla` and `formal-kani`:
[run 43812934865](https://github.com/seek-hope/TeamAgents/actions/runs/43812934865).
A later scheduled run on the same source, [43868995516](https://github.com/seek-hope/TeamAgents/actions/runs/43868995516),
failed `update_json_with_a_live_daemon_prints_only_valid_json_on_stdout` at daemon readiness; the other three
jobs passed. Neither run covers D-526/D-527 or the Unicode search fix. Subsequently, CI on `a4ce2a2e` passed all
four jobs in [run 44000392754](https://github.com/seek-hope/TeamAgents/actions/runs/44000392754), before D-528.
The 26 AArch64 tool tests run under GNU user emulation, not native ARM.
Package-baseline checks cover all 18
offline probes, source/installed PTY matrices, x86_64 musl release/install, scripted profile/fetch behavior
and a separate real Fetch API request. These bindings identify the tested source; subsequent edits do not
inherit a new CI pass or installation result merely by preserving the counts.
The current local formal replay passed **63 positive configurations in 60 traversals**, **323 controls**,
**79 witnesses** and **9 Kani harnesses**, with all four Make targets exiting 0. Its input-content binding is in
[the formal report](../verification/REPORT.md), separate from the installed-runtime CI above.
The catalogue contains 387 configurations across 47 modules, including the
separate wide configuration. PTY covers five terminal profiles and both forced graphics modes; this is not a
complete graphical-terminal or OS matrix.

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
| Q18 | Old config/session compatibility is not inherently required; migration and cleanup are deliberate | A34, A36: schema 9 migration and explicit cleanup are implemented; older writers cannot reopen the migrated store |
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
| A36 | Store/session lifecycle, search/export and guarded cleanup | `engine/tests/session_browser.rs`, `engine/tests/session_workspace.rs`, `session_export_tests.rs`, session CLI, `tools/install_check.py`, current schema 9 and maintained development tools |

## Current runtime and installation evidence

D-528 fixes the default test gate's attribution of a concurrent session as a leak. Signals bind the selected
PID/start identity to a pidfd and recheck ownership; test scratch roots are recorded in a per-gate registry.
Lost registry evidence, malformed snapshot bindings and invalid markers fail visibly. Peer registries are not
aged out, so historical residue can accumulate across runs. The original field-debug `OUTCOME_UNKNOWN` remains
unresolved; this repair does not recover that command's outcome.

TeamAgents implemented candidate `ec1f1acb` and passed its final `make check`: 855 Rust tests, formatting,
Clippy and hygiene. The supervisor inspected the actual operation receipt and separately passed 19/19 targeted
registry controls. Those independent controls used real filesystem fixtures with mocked stop/kill functions;
they do not by themselves prove real-process termination. Merge `cab1c47c` has the same tree as that candidate.
The supervisor then passed `make test-aarch64` on the merged source: 26 tool tests under GNU/QEMU, not native ARM.
The formal replay remains bound to the material hash in [the formal report](../verification/REPORT.md);
subsequent registry follow-ups did not change those formal inputs.

Both installed binaries were built from `cab1c47c` with the pinned toolchain, `--offline --locked --release`,
`--jobs 4` and target `x86_64-unknown-linux-musl`. SHA-256:

- CLI: `242153aa12df04642bf9be94b71417bef0befe0566b3170a6ed227847062196c`.
- TUI: `196ff1661a8dffa95bbe4f3eb907e3c9dbf933e2f447afeaf9981a907f1d25a2`.

Atomic replacement kept durable backups and preserved all four pre-existing daemon PID/start identities and
mapped executable inodes. Running daemons retained their prior binaries. Installed version/help, isolated
init/doctor, four Unicode search hits with an unchanged disposable database, and seven real-terminal key/editor
checks passed. The minimal doctor fixture intentionally configured no optional skills, web tools, work limits or
completion checks; its optional warnings were retained. No model calls or ordinary session access occurred.
The older five-profile PTY matrix, session-switching and complete-package evidence were not rerun or renewed.

Local evidence root: `~/.local/state/teamagents-runtime-debug-17bo3o4u/`. Independent checks and the delegated
receipt review are `registry-final-independent-acceptance.json` and `registry-final-receipt-review.json`.
`delivery/` contains `build.json`, `installation.json`, `test-aarch64.json` and the installed probe logs/receipts.
The backup directory is `teamagents-leak-install-uvvvtn6_` under that evidence root. These local paths are provenance,
not tracked dependencies. Reproduction entry points are `make check`, `make test-aarch64`,
`python3 tools/leak_guard.py --self-check` and `engine/tests/leak_registry.rs`.

The Unicode search fix in `c1923244` translates lowercase-string match offsets into original-character
boundaries, preserving the original-text snippet window. Independent review reproduced the old installed
CLI panic on a disposable database, then verified four matching messages through debug, musl release and
installed CLI binaries without changing the database. A separate probe using the production function checked
every Unicode scalar as a prefix, repeated length-changing characters, contextual sigma and a match inside a
lowercase expansion. The full local `make check` and `make language-check` passed on that source: 854 Rust
tests (198/535/121), formatting, Clippy, hygiene and no process leak. The CLI was atomically replaced after
a durable private backup; all three pre-existing daemons retained their PID/start identities. At that delivery,
TUI inputs and its installed hash were unchanged, so its D-527 PTY evidence remained scoped to that source. No new package,
clean-host or formal acceptance is claimed. Local receipts are under `tools/tmp/field-review/`.

TeamAgents found and implemented the fix in a separate real-model run. Its execution remained blocked by one
unknown outcome and stale running records; accepting the independently verified patch does not establish
successful autonomous task finalization. No model calls were made by the independent acceptance probes.

D-527 adds scoped CLI validation and help, explicit end-of-options parsing, arbitrary MCP argv values,
mutually exclusive update actions, resolved permission overrides and structured JSON mutation receipts.
`engine/tests/interface.rs` checks isolated refusal before state access, guarded deletion and the new output
contracts; parser tests keep command names and dash-leading prompts literal after `--`. `tui/tests/interface.rs`
covers modifiers, Repeat events, approval identity across refresh, explicit editor syntax and shared slash
metadata. Default bindings, Leader targeting, drafts and existing capabilities remain available. The PTY
smoke waits for the visible composer acknowledgement between Esc and F6 after resizing; fixed delays can
coalesce those bytes in the terminal parser. These interface changes do not alter schema or formal inputs.
The D-527 local gate passed all 852 Rust tests, formatting, Clippy and hygiene. The source TUI passes five
terminal profiles and both forced graphics modes. Each release/installed pair passes 23 isolated CLI
invocations and seven PTY checks covering approval modifiers/repeat, refresh, profile repeat, attachment paths,
incomplete-command usage and explicit editor launch. Installed session switching also passes in a real PTY.
The binaries were replaced atomically after private backups; the existing user daemon retained its PID/start
identity. No ordinary configuration/session files were written and no real model was called by these probes.
This does not renew the complete package, CI, external-provider or clean-host acceptance above.

| Installed binary | SHA-256 |
|---|---|
| CLI | `dfb2d36479fec39cf126487f3780871c5e463c56dbc0ef7331d40f4ad343b30a` |
| TUI | `e08424dd22ba6378f2bff5a0c90125da8549a68f93c914c275baa8b2ead64f8f` |

Workspace-session coverage (D-526) includes current-directory selection, successful-open recency, absolute
path persistence, cold restart in the saved workspace, unavailable-candidate isolation and concurrent creation.
`engine/tests/session_workspace.rs` uses isolated state and an unavailable local model endpoint;
`tools/dogfood/tui_sessions.py` verifies workspace changes and per-session drafts in a real PTY.

The project config is read under explicit user trust (D-244). Loader regressions and
`tools/dogfood/project_config.py` cover trusted/untrusted repositories and refusal of project self-authorization.
Permissions, hooks, checks, limits, retention and update policy remain user-controlled.

Named-profile resolution applies before first-request preparation and during recovery, preserving the chosen effort
and unchanged cached retry requests. The driver regression follows a transient failure through retry, tool use
and subsequent model calls; the crash test reopens a named-profile request with honest unknown-usage accounting.
Cross-provider switching and ambiguous profiles sharing a wire model identity remain open.

Team-interface regressions cover split-transcript media and real-terminal member-history failure/retry. Navigation keeps Leader input, interruption and profile targeting; member history, previews and media stay separate from
the Leader transcript. Tiny/narrow/wide layouts, mouse selection, retained drafts, approval ownership, task
outcomes and bounded attachment-local activity are covered. The five-profile PTY matrix and forced graphics
modes pass, including simultaneous Leader/member media and hidden-task polling controls. The package-baseline 837-test
local gate and all 18 offline probes pass on 2026-11-16; the real-daemon panel probe preserves a draft while
inspecting a member and observing task cancellation. This establishes scripted behavior, not measured usability
or live team quality.

DNS/fetch regressions cover independently verified public hostnames behind fake-IP DNS. Both configured
search providers work on the current host.
Private literals and mixed private answers remain refused. A live GitHub fetch returns 4096 bytes under a
4096-byte limit. Broader proxy environments and provider-side destination/redirect enforcement remain open.

The `d96d7c8a` runtime passed a retained x86_64 Linux musl release/install rehearsal and the installed five-profile
PTY matrix with both forced graphics modes. The installed CLI completed a scripted profile/retry/tool/fetch
workflow: all five requests, including the retry, used the resolved wire model and selected effort; the retry
body stayed unchanged, public fetch used its canonical URL, and metadata-address fetch was refused. A separate
installed run fetched GitHub through the real DeepSeek Fetch API and returned 4096 bytes. Both runs stopped their
owned daemons/runners. Neither run used a real model. These checks cover this host, not fresh-host or ARM-package
acceptance.

The complete package archive SHA-256 is `c59e71935621454911beb6c3aad9ed342fbb3b36455878ee5b9dd82fb0431e7f`;
its CLI is `d47d96262479de23aa26719dbe5643618f559395d6db6bb2c2153f0ea8edd8d9`, and its TUI is
`47a45e21a2437fbbcf8f614325977af2edab884528eec5514b22aa8d5b78183c`. That TUI remains installed; the installed
CLI is the D-526 binary identified above. The version remains 0.2.0. No new public release was published.

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
- Cross-provider profile switching/shared wire-name ambiguity, provider-side fetch DNS/redirect/rebinding,
  broader proxy configurations, session startup/filesystem races and full-backlog connection deadlines remain open.
- Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition, arbitrary-string
  capability algebra and Policy B source-bound bidirectional fair-trace equivalence remain unestablished.
- Scheduler/provider fairness, escaped descendants, remote effects and real power-loss timing remain
  environmental obligations. Cancellation and process absence do not reconstruct unknown outcomes.
- MCP revision `2025-06-18` has local executable coverage; external OAuth/SDK/client/extension matrices,
  resumable streams and broader versioned integration contracts remain open.
- Artifact references remain protected after owner release; no destructive reference-expiry policy is selected.
  Artifact lifetime under future retention policies and broader dependency ecosystems remain open.
