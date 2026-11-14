# Development handoff

## Current baseline

Last committed source baseline: `3274d1fa`; verification pin: `9b012df9`. Recheck Git for subsequent work.

The current source implements the requirements in [DESIGN.md](DESIGN.md), the rules in [DECISIONS.md](DECISIONS.md),
and the acceptance matrix in [ACCEPTANCE.md](ACCEPTANCE.md). Current formal evidence and its limits are in
[verification/REPORT.md](../verification/REPORT.md).

Current source inventory and verification:

- **814 Rust tests**: core 197, engine 509, TUI 108; the full `make check` passed.
  All 18 offline probes and the final release/install rehearsal passed. PTY passed on the unchanged TUI/daemon
  code. The final archive passed isolated upgrade and rollback on the existing Debian host (see ACCEPTANCE.md).
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
| TUI and terminal behavior | `tui/tests/minimal.rs`, `tui/tests/conversation.rs`, `tui/tests/parity.rs`, `make pty` |
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

- Small live trials cover 8 paired Pi tasks, 34 reused DeepSeek Reviewer cases, multi-branch tree recovery
  and native-window compaction. Claude matched 24/34 labels with four parser and five quota failures; do not
  retry its exhausted paid route without a resource change. General success, tree semantics and user lostness remain open.
- Four wire protocols have live evidence. Earlier pinned builds passed public SSH/screen reconnect and isolated
  install/upgrade/rollback on an existing Debian 12 host; external OAuth, fresh-host and wider platform acceptance remain open.
- Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
  equivalence remain open.
- Broader versioned integrations, artifact-retention policy and general long-horizon outcome measurement remain open.
