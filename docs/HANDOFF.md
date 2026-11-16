# Development handoff

## Current baseline

Runtime source baseline: `d96d7c8a` (D-523–D-525). D-523 resolves named profiles before request preparation and
recovery, preserving first-request effort and unchanged cached retries. D-524 adds a Herdr-inspired member
sidebar, inspection/history, tasks and activity while keeping input, interrupt and profile controls on Leader.
All fifteen slash commands share the same completion/guidance registry. D-525 verifies public fetch destinations
behind fake-IP DNS and enforces UTF-8 byte limits; search providers already worked on this host.

The full 837-test local gate, the five-profile PTY/graphics matrix (including split-pane media and hidden-view
polling), and all 18 offline probes pass. The isolated executable workflow also covers selected effort before
the first request, frozen retries, tool follow-ups, canonical public fetch and private-address refusal.
The retained x86_64 musl package passes its release/install rehearsal and installed PTY matrix. Its installed
CLI passes the scripted profile/fetch workflow and a separate live GitHub fetch through the current host's DNS
and DeepSeek Fetch API; these are not real-model quality trials. The ordinary CLI/TUI installation now matches
this runtime, with a fresh private backup and unchanged configuration/state files. No ordinary TeamAgents
processes were present at installation, and no session was started or resumed. No session database was present
in the ordinary state tree, so this is not an ordinary existing-database preservation or migration claim.
Cross-provider hot switching, ambiguous wire identities and provider-side fetch redirect enforcement remain open.

Previous runtime source baseline: `a6b5555a` (D-522). It protects session archive/restore/delete/prune when an endpoint
is listening or absence is uncertain.
Explicit stop propagates failures and uses the protocol-1/2 maintenance connection. Twenty lifecycle and
eight stop-safety conditions, the old-version shutdown fixture and the real CLI prune probe pass locally;
startup/filesystem races and a full-listen-backlog deadline remain outside this repair. Its full 819-test gate,
PTY matrix, 18 offline probes and musl release/install rehearsal passed. The installed package also passed
batch-edit refusal/repair and session-preservation checks; exact artifact identities are in ACCEPTANCE.md.

The `675af52e` runtime (D-521) uses target-ABI file-open constants to fix AArch64 EINVAL
failures without changing x86 flag values. Its musl release/install rehearsal and installed hash workflow
passed; the AArch64 GNU daemon/headless workflow also passed under user emulation.
The full local 817-test gate and all 26 tool tests under AArch64 GNU user emulation passed. The shared
`make test-aarch64` target has a separate CI job; native ARM hardware/kernel and package acceptance remain open.
The full formal chain was re-run against `675af52e`, including all 455 material-file bindings; the separate
90-minute wide attempt timed out and remains incomplete. The previous `fde93f53` delivery passed all three
of its CI jobs. The `ab325b59` runtime (D-520) passed its
release/install rehearsal and installed hash-refusal/repair workflow. Fresh Debian/Ubuntu VM acceptance and
their associated offline receipt remain pinned to `0eb4ee98`. D-519 audits current documentation totals. Recheck Git and CI
for subsequent work; exact package and experiment identities are in ACCEPTANCE.md.

The current source implements the requirements in [DESIGN.md](DESIGN.md), the rules in [DECISIONS.md](DECISIONS.md),
and the acceptance matrix in [ACCEPTANCE.md](ACCEPTANCE.md). Current formal evidence and its limits are in
[verification/REPORT.md](../verification/REPORT.md).

Current source inventory and verification:

- **837 Rust tests**: core 198, engine 522, TUI 117; the full `make check` passed on 2026-11-16.
  D-521 also passes the 26-tool AArch64 GNU emulation subset and a scripted daemon/headless workflow.
  D-522 passed the PTY matrix as it existed at `a6b5555a`, 18 offline probes, musl release/install and installed workflow/session
  preservation checks; D-521's architecture evidence remains bound to its own source.
  The earlier `0eb4ee98` archive passed 18 offline probes and fresh Debian/Ubuntu VM installation, malformed-
  argument rejection, upgrade, rollback and failed-startup recovery (see ACCEPTANCE.md).
- **65 positive TLA configurations in 62 traversals**, **327 controls**, **79 witnesses**, **9 Kani harnesses**.
- Catalogue: **393 configurations across 48 modules**, including the separate wide configuration.
- PTY covers five terminal profiles and both forced graphics modes.

These are local scripted, finite-model and real-terminal results, not full real-service acceptance.

## Start here

1. Read [AGENTS.md](../AGENTS.md), then inspect:

   ```bash
   git status --short --branch
   git log -5 --oneline
   ```

2. Read the current rules in [DESIGN.md](DESIGN.md) and [DECISIONS.md](DECISIONS.md).
3. Use [DEVELOPMENT.md](DEVELOPMENT.md) for commands and [ACCEPTANCE.md](ACCEPTANCE.md) for evidence/open work.
4. Keep code and documentation English, except `README.zh-CN.md`.
5. Confirm any design deviation before implementation and record only the resulting current rule.

## Current behavior

- `Control::submit` owns atomic team transactions; SQLite WAL and `synchronous=FULL` own durable facts.
- Work scopes own budget, deadline, admission and acceptance. Work states/tasks own continuation and attribution.
- Recovery reuses known results and records unknown effects as `OUTCOME_UNKNOWN`; side effects are not replayed.
- Malformed ordinary tool arguments fail before dispatch, with raw calls and usage preserved. Valid sentinel-key
  data is accepted; old PREPARED rows use available stored source calls to distinguish parser failures.
- Batch file edits reject malformed supplied hash preconditions before writing any file. Valid hexadecimal
  hashes remain case-insensitive, and omitting a hash retains the existing behavior.
- Cancelling a task does not prove its process stopped. Instance termination is user-only and needs separate
  stop evidence before shared-workspace takeover.
- One daemon serves each session. TUI and exec resume from the event watermark and share the same runtime.
- Shell, acceptance commands and local MCP run on the host with capability checks, not OS confinement.
- One session-level exploration tree accompanies Leader compaction. It is model-maintained navigation, not
  authority, completion evidence or an alternate history. Member-private history is excluded.
- `/tree`, `teamagents search_tree [--json]` and export expose the same tree. Ordered patches reject atomically
  and replay without growth. Invalid compaction patches leave the old tree stale while summary and work continue.
- Current development and verification utilities live under `tools/`. There is no `review/` data dependency.

## Regression entry points

| Area | Current entry point |
|---|---|
| Work, budgets and migration | `core/src/v2/control/work_tests.rs`, `core/src/v2/store.rs` |
| Driver and compaction | `engine/tests/v2_driver.rs`, `engine/tests/cases/autonomy.rs` |
| Search-tree control and replay | `core/src/v2/control/search_tree.rs` |
| Daemon, sessions and headless execution | `engine/tests/v2_daemon.rs`, `engine/tests/session_browser.rs`, `engine/src/v2/session_export_tests.rs` |
| Provider and runner robustness | `engine/tests/cases/provider_robustness.rs`, `engine/tests/cases/jobs_robustness.rs` |
| TUI and terminal behavior | `tui/tests/team.rs`, `tui/tests/minimal.rs`, `tui/tests/conversation.rs`, `tui/tests/parity.rs`, `make pty` |
| Fake-IP DNS and fetch | `engine/src/web_url.rs`, tool/search tests and D-525's scoped host probes |
| Executable control-plane correspondence | `core/tests/v2_invariants.rs` |
| Formal search-tree model | `verification/tla/V2SearchTree.tla` and `verification/witnesses.json` |
| Document and tool generation | `tools/*_catalogue.py`, `tools/config_reference.py`, `tools/citations.py` |

## Verification commands

```bash
make check
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
make pty
```

Accept a result only when the actual target exits 0 and its requested root is processed without hidden warnings,
omitted proofs or aborts. A witness is reachability evidence, not safety proof. A skipped or timed-out check is
not rewritten as a pass.

## Open work

- Small live trials cover the older 8 paired Pi tasks, a six-run repository pilot with unequal Cargo caches,
  an interrupted disk-full cohort, and a completed six-run comparison with matched build configuration. In the
  matched cohort, Pi passed 2/2 hidden suites, single Leaders 0/2 and teams 1/2; both teams timed out and both
  single Leaders incorrectly reported success. All 83 agent-added tests across the first and third cohorts
  passed independent updater checks. PRODUCT-COMPARISON.md records the distinct evidence and limits.
- DeepSeek Reviewer evidence covers 34 reused cases; Claude matched 24/34 labels with four parser and five quota
  failures. Do not retry its exhausted route without a resource change. Multi-branch recovery and native-window
  compaction have scoped evidence; general success, tree semantics and user lostness remain open.
- Four wire protocols have live evidence. Earlier pinned builds passed public SSH/screen reconnect and isolated
  install/upgrade/rollback on an existing Debian 12 host. The `0eb4ee98` candidate passed fresh Debian/Ubuntu
  VM checks with scripted loopback providers. D-521 adds scoped AArch64 GNU user-emulation coverage; external
  OAuth, physical hosts, ARM kernels and wider platform acceptance remain open.
- Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
  equivalence remain open.
- Broader versioned integrations, artifact-retention policy and general long-horizon outcome measurement remain open.
