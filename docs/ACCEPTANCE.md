# Current acceptance matrix

Baseline: [DESIGN.md](DESIGN.md) §1/§12 and the current decisions in [DECISIONS.md](DECISIONS.md).
This file records current behavior and reproducible evidence, not historical acceptance runs.

✅ means the named local path has automated evidence. 🔶 means implementation exists but the stated external,
empirical or full-system obligation remains open. No icon is a proof of every release condition.

Current source inventory: core 198 / engine 522 / tui 117 test targets; the full gate passed.
The `a6b5555a` runtime passed all 18 offline probes (154.3 seconds), all PTY profiles/graphics modes and
the release/install rehearsal. Its installed package passed the scripted batch-edit workflow and session
preservation probe below. Fresh Debian/Ubuntu VM acceptance remains pinned to `0eb4ee98`.
Current formal gate: **65 positive configurations in 62 traversals**, **327 controls**, **79 witnesses** and
**9 Kani harnesses**, all exit 0. The catalogue contains 393 configurations across 48 modules, including the
separate wide configuration. PTY covers five terminal profiles and both forced graphics modes; this is not a
complete graphical-terminal or OS matrix.

## Requirements (Q1–Q19) and their evidence

| # | Current requirement | Current evidence and boundary |
|---|---|---|
| Q1 | Task success and long-horizon reliability matter equally at the same model and budget | A01, A10, A16, A35: persistent work, bounded recovery, required checks and usage accounting are implemented; general live-model success rates remain unmeasured |
| Q2 | Language and component boundaries may change | `rust-toolchain.toml`, Rust `core`, `engine` and `tui` crates; test-only Python fake services and probes |
| Q3 | Kernel and runtime were delivered first | A01, A33: `core/src/kernel`, `core/src/v2`, `engine/src/v2` and the daemon/driver/supervisor path |
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
| A36 | Store/session lifecycle, search/export and guarded cleanup | `engine/tests/session_browser.rs`, `session_export_tests.rs`, session CLI, `tools/install_check.py`, current schema 9 and `tools/` relocation |

## Current repair evidence

D-523 resolves named profiles before first-request preparation and during recovery, preserving the chosen effort
and unchanged cached retry requests. The driver regression follows a transient failure through retry, tool use
and subsequent model calls; the crash test reopens a named-profile request with honest unknown-usage accounting.
Cross-provider switching and ambiguous profiles sharing a wire model identity remain open.

D-524 adds eight team-interface regressions, a split-transcript media regression and real-terminal
member-history failure/retry checks. Navigation keeps Leader input, interruption and profile targeting; member history, previews and media stay separate from
the Leader transcript. Tiny/narrow/wide layouts, mouse selection, retained drafts, approval ownership, task
outcomes and bounded attachment-local activity are covered. The five-profile PTY matrix and forced graphics
modes pass, including simultaneous Leader/member media and hidden-task polling controls. The full 837-test
local gate and all 18 offline probes pass on 2026-11-16; the real-daemon panel probe preserves a draft while
inspecting a member and observing task cancellation. This establishes scripted behavior, not measured usability
or live team quality.

D-525 adds eight DNS/fetch regressions. Both configured search providers work on the current host; fake-IP
DNS previously blocked GitHub fetches locally and now permits independently verified public hostnames.
Private literals and mixed private answers remain refused. A live GitHub fetch returns 4096 bytes under a
4096-byte limit. Broader proxy environments and provider-side destination/redirect enforcement remain open.

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

## Scoped external and live receipts

D-522 preserves sessions when endpoint absence is unproven. A real CLI call in an owned synthetic fixture
previously pruned an archive despite an open malformed-greeting listener and held coordinator lock. The
repaired call retains the row and marker. Two Rust regressions cover 20 lifecycle conditions and eight
default/named stop-safety conditions, including legacy greetings, invalid greetings, uncertain paths,
stale/missing sockets, explicit shutdown rejection and uncertainty after acknowledgement. Existing protocol-1 shutdown and late-workspace
checks pass. These are local endpoint/SQLite checks; they do not establish full startup/filesystem-race safety
or a connection deadline for a full socket backlog. Inputs and receipts are retained under
`tools/tmp/session-liveness-evidence/`.

The `a6b5555a` x86_64 Linux musl archive has SHA-256
`3567c44433878194b34e5bd3471e9d109a7a0a1ad00795e123dba916d065da8f`; its CLI has SHA-256
`ac43e46501dc951b42c87e835c3780e77f3a95cf225f995695b9be36b7b3ad38`. It passed isolated installation,
repeated-init configuration preservation, batch-hash refusal before a successful repair, and a CLI prune
check that retains the registry/marker while a malformed listener and coordinator lock are held. The scripted
work completed with `SUCCEEDED`; daemon/runner cleanup succeeded and the listener thread stopped. All 16
packaged Markdown files match that source. The first follow-up attempt failed before installation because
the default rehearsal had removed its temporary archive; a separate `--keep` rehearsal supplied this artifact.
Both attempts and exact inputs remain under `tools/tmp/d522-installed-evidence/`. This is local scripted
acceptance, not a new VM/live-provider result, normal-host installation replacement or public release.

D-521 corrects target-ABI file-open flags. On AArch64, the old x86 `O_DIRECTORY` literal selected `O_DIRECT`,
causing seven of the existing 26 tool regressions to fail with EINVAL. All 26 pass after using the existing
`libc` constants; the full local 817-test gate also passed. The CLI and TUI cross-build for AArch64 GNU.
`make test-aarch64` and its separate CI job retain the regression subset with an explicit QEMU runner.
These are user-emulation checks on the existing x86_64 Linux kernel, not native ARM or package acceptance.
The local GNU binaries use the cross-toolchain sysroot (CLI symbols through GLIBC 2.44, TUI through 2.43);
they are not portable ARM release artifacts. Before/after logs and build provenance are retained under
`tools/tmp/aarch64-evidence/`. The earlier `fde93f53` delivery passed all three of its CI jobs.

The fixed AArch64 GNU CLI at `675af52e` also passed the scripted daemon/headless workflow: malformed
hashes failed with started=true and unchanged files, then matching hashes edited both files and the work
ended SUCCEEDED. Daemon/runner cleanup exited 0 with no runners or provider errors. Its CLI SHA-256 is
`91ad24842529e2189c4a36d2c63838e02c3dcc20af6aa0cce658f6cade431b24`; the exact inputs, before/after operation
receipts and source binding remain under `tools/tmp/aarch64-evidence/`. An earlier probe keyword error is
preserved separately from the reproduced EINVAL failures.

The D-521 x86_64 Linux musl candidate was packaged from `675af52e`. Archive SHA-256:
`80056db6b3ee9f91e9a095c27975d944d471ee0b0f4cc79256815c814236e870`; CLI SHA-256:
`f94a1c1e10cafc6d024a4cd6fc8866b70aea37b5af6773b8c6cb80cc8c63aa3b`. The release/install rehearsal, repeated-init
configuration preservation and the installed hash-refusal/repair workflow passed on the existing x86_64 host.
All 16 packaged Markdown files match the package source. Inputs and receipts are retained under
`tools/tmp/d521-installed-evidence/`. This remains a local candidate and does not renew fresh-VM, native ARM,
live-provider or public-release acceptance. The 446 retained formal inputs remain byte-identical.

The D-520 candidate was packaged from `63c98851`, with runtime code unchanged since `ab325b59`.
Its x86_64 Linux musl archive has SHA-256 `fcde7164b78f4fe7a66f129656fcb409cfcfe43262590f0ca8e10bf2d5d1bd85`;
its CLI has SHA-256
`5e0049588efe03d9beaf4458d8b4827b005db7a3575d7a9558c025a05b13e880`. The full 817-test gate, release/install
rehearsal and an installed-binary loopback workflow passed on the existing Linux 7.3.3 x86_64 host. A malformed
per-edit hash produced FAILED/started=true with both files unchanged; corrected hashes then produced a successful
batch and SUCCEEDED work. The scripted provider checked the unchanged files before issuing the repair. Daemon
and runner cleanup exited 0, with no runners listed. Inputs and receipts are retained under
`tools/tmp/d520-installed-evidence-v2/`; the first archive and its separate receipt remain preserved. The second
package includes the README's explicit older VM-build attribution. This does not renew the older fresh-VM or
live-model results.

The `0eb4ee98` candidate includes D-517/D-518 and predates D-520. Its x86_64 Linux musl archive has SHA-256
`568ddb813efd124c9e401e0df75f9f779bd7796c37cf3a1df478f947716d6d3d`; its CLI has SHA-256
`2666b881f1401341cbd620941991e315e234724791bab2e2f168fd5dba61b838`. All 18 offline probes passed (154.0 seconds),
and the release rehearsal built both binaries, checked the package and preserved configuration across repeated init.

Two fresh official cloud-image KVM guests passed ordinary-user installation and real Shell execution:

| Guest | Observed kernel | Result |
|---|---|---|
| Debian GNU/Linux 12 | `6.1.0-54-cloud-amd64`, x86_64 | Install, argument rejection, upgrade and recovery passed |
| Ubuntu 22.04.5 LTS | `5.15.0-203-generic`, x86_64 | Install, argument rejection, upgrade and recovery passed |

Each guest had a new cloud-init identity, a private overlay, no host filesystem mount and a scripted loopback
provider. Malformed `ls` arguments produced a failed, not-started receipt; a valid `_invalid_arguments` property
succeeded in the same batch. That rejection workflow intentionally ended FAILED/exit 1. A separate real Shell
workflow ended SUCCEEDED. Managed upgrade, explicit rollback, re-upgrade and an injected exit-42 startup failure
followed by automatic rollback preserved configuration bytes and all six original history entries. The injected
upgrade returned the expected exit 1; both enclosing sequences exited 0. Scoped native-process scans were empty,
both guests shut down with QEMU exit 0, and base-image hashes stayed unchanged. Official HTTPS checksum files
matched the images; detached signatures were not checked.

These transitions used two distinct 0.2.0 builds with schema 9/protocol 2. They do not establish schema migration,
physical-host behavior, broader architectures, live-provider quality or absence of arbitrary escaped descendants.
The package remains a local candidate. Raw reports, input manifests, fixture sources and shutdown receipts are
retained under `tools/tmp/fresh-vm-debian-evidence/` and `tools/tmp/fresh-vm-ubuntu-evidence/`. Private keys,
seeds and disks remain outside the repository.

The earlier accepted archive has SHA-256 `951947815346f195fbf7a91091cf1089b9d55093498b69faf3f819a2384f223d`,
with CLI SHA-256 `ae6aa27af25799e7f42fbe67c4e503d34c77bd593580f39687417a69d7ef526e`. It predates D-517/D-518,
served as both guests' upgrade baseline, and was the runtime used in all three repository-comparison cohorts.
Existing-Debian upgrade/rollback and public SSH/GNU screen results retain their separately pinned builds.

The portable Reviewer probe's fixed-case DeepSeek rerun matched 34/34 labels with no operational errors at the
reported native 1,048,576-token window. It records source, executable and input hashes; the same cases were used
in V4, so this adds reproducibility, not independent holdout coverage. Earlier Claude parser/quota failures remain
part of the evidence below. No operation described by these assessment cases was executed.

## Current open obligations

- The project config is read, since D-244, only through explicit project trust; repository-local hooks, checks,
  limits, retention and update policy remain user-only.
- The older 8-run paired `deepseek-flash` comparison at native 1M passed 8/8 hidden checks on both clients.
  A newer known-repository regression passed 2/2 hidden suites with Pi and 0/2 each with a single Leader and
  a one-member team; its Cargo caches were unequal, so timings are not matched evidence. A second cohort
  stopped on temporary-disk exhaustion before grading its first run. A third cohort matched Cargo settings:
  Pi passed 2/2, single Leaders 0/2 and teams 1/2; both teams timed out, and both single Leaders incorrectly
  reported success. Independent updater checks passed all 83 agent-added tests across the completed cohorts.
  Full results and limits belong in [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md); broader held-out SWE-scale
  benchmarks remain open.
- Hardened `reviewer.rs` (`hard_deny` + untrusted delimiters) passed **34/34** cases on `deepseek-flash` after
  the literal read-only search correction (V4, reused fixed cases, not a fresh holdout); the earlier 22-case run had one deterministic quoted-publication false
  refusal. Claude V3 matched 24/34 expected labels, with four strict-output parser failures and five HTTP 402 quota
  failures, not correct decisions. No dangerous allow was observed among completed assessments; this is not broad
  safety proof. Multi-model/SWE-scale adversarial coverage, semantic tree completeness and larger empirical
  lostness studies remain open.
- All four wire protocols (`deepseek`, `chat/completions`, `anthropic`, `responses`), heterogeneous
  `deepseek` + `anthropic`, search/fetch, `deepseek-flash` images, loopback SSH, public SSH/GNU screen reconnect,
  Alacritty and isolated existing-Debian install/upgrade/rollback passed on earlier pinned builds. The
  `0eb4ee98` candidate passed fresh Debian/Ubuntu VM checks; external OAuth services, physical hosts and broader
  terminal/OS distribution matrices remain open.
- The wide exhaustive configuration remains incomplete after the unchanged 90-minute attempt; an earlier
  simulation explored 2,029,647 states but was not exhaustive. Full Rust/SQLite refinement, whole-system
  composition, arbitrary-string capability algebra and Policy B source-bound bidirectional fair-trace
  equivalence remain open.
- Scheduler/provider fairness, process/remote effects, filesystem races and real power-loss timing remain
  environmental obligations.
- MCP resource list/read, resource-only and filtered-tool bindings, paged catalogues, protocol revision refusal,
  string request IDs, OAuth initialization refresh and local credential isolation pass. The supported revision is
  `2025-06-18`; external OAuth, SDK/client extension matrices, resumable push streams and broader integration
  contracts remain open.
- Artifact collection protects operation receipt and context references after owner release, retries failed
  deletion and skips `EVIDENCE`; no new reference-expiry policy is selected. Artifact lifetime under all future
  retention policies and general integration coverage remain open.
