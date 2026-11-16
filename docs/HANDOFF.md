# Development handoff

## Current baseline

The project is a standalone Rust control plane, runtime and team TUI. Read [DESIGN.md](DESIGN.md) for the
Q1–Q19/A01–A36 baseline, [DECISIONS.md](DECISIONS.md) for current contracts and [ACCEPTANCE.md](ACCEPTANCE.md)
for exact source-bound evidence. Git retains implementation and experiment history; do not recreate a changelog here.

Installed runtime: `d96d7c8a`. The recorded delivery CI at `6767b32c` passed all four jobs. Current source
adds workspace-based recent-session selection (D-526); its local regression coverage is recorded below.
Source changes do not rebuild the installed runtime or automatically renew that CI result; inspect Git before
relying on any baseline. Current-host packaging/installation and scripted checks pass; no user session was started or
resumed during installation. The ordinary state tree then contained no session database, so no existing-database
migration/preservation result is claimed. The current local formal replay passes with its input-content
fingerprint recorded in [verification/REPORT.md](../verification/REPORT.md).

- **841 Rust tests**: core 198, engine 526, TUI 117.
- **63 positive TLA configurations in 60 traversals**, **323 controls**, **79 witnesses**, **9 Kani harnesses**.
- Catalogue: **387 configurations across 47 modules**, including an incomplete wide configuration.
- Source and installed PTY matrices cover five terminal profiles and both forced graphics modes; 18 offline
  probes, installed scripted profile/retry/tool/fetch checks and a separate live Fetch API check pass.
- AArch64 coverage is the 26-tool GNU user-emulation subset, not native ARM/kernel/package acceptance.

These checks do not establish real-model task quality, full real-service acceptance or native-runtime refinement.

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
- Malformed argument encoding, JSON or non-object root arguments fail before approval/dispatch with
  `started=false`, raw calls and usage preserved. Valid sentinel-key data is accepted; PREPARED rows use
  available stored source calls to distinguish parser failures. This is not full per-tool schema validation.
- Malformed batch-edit hash preconditions fail in the tool body with `started=true`, before staging or writes.
  Valid hexadecimal hashes remain case-insensitive; omitted hashes remain supported.
- Cancelling a task does not prove its process stopped. Instance termination is user-only and needs separate
  stop evidence before shared-workspace takeover.
- One daemon serves each session. TUI and exec resume from the event watermark and share the same runtime.
- Ordinary TUI/exec startup restores the most recently opened session in the canonical launch workspace,
  creating one if absent. Explicit session/root selection and TUI switches retain the saved workspace. Cold
  starts load project configuration from that workspace. Successful opens update recency; reads do not.
  Unusable automatic candidates produce a note and are skipped; explicit opens retain actionable failures.
  `engine/tests/session_workspace.rs` covers selection, restart, path persistence, recency and concurrent creation;
  `tools/dogfood/tui_sessions.py` checks real-terminal workspace switching with retained drafts.
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
| Fake-IP DNS and fetch | `engine/src/web_url.rs`, tool/search tests and scoped host probes |
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

[PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) is the candidate-work queue. Its priorities are matched held-out
outcome/cost comparisons, reliable team completion, independent Reviewer evaluation, tree/navigation usefulness,
provider/OAuth/integration coverage and current clean-host/platform acceptance.

Specific runtime gaps include cross-provider switching and ambiguous wire identities, provider-side fetch
redirect/rebinding enforcement, session startup/filesystem races and full-backlog connection deadlines.
Wide exhaustive completion, Rust/SQLite refinement, whole-system composition and Policy B source-bound
bidirectional fair-trace equivalence remain open. Fair scheduling, escaped/remote effects and real power loss
remain environmental obligations. Retention policy does not authorize expiry of referenced output bytes.
