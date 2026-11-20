# Development handoff

## Current baseline

The project is a standalone Rust control plane, runtime and team TUI. Read [DESIGN.md](DESIGN.md) for the
Q1–Q19/A01–A36 baseline, [DECISIONS.md](DECISIONS.md) for current contracts and [ACCEPTANCE.md](ACCEPTANCE.md)
for exact source-bound evidence. Git retains implementation and experiment history; do not recreate a changelog here.

Installed CLI and TUI source: `cab1c47c`, merging the independently accepted `ec1f1acb` leak-guard candidate.
Both binaries were rebuilt with the pinned toolchain for x86_64 Linux musl and installed by atomic replacement
with durable private backups. All four existing daemons retained their PID/start identities and mapped executable
inodes; they continue running their prior binaries. Installation probes used disposable configuration/state and
made no model calls: version/help, init/doctor, Unicode search and seven real-terminal key/editor checks passed.
The full five-profile PTY matrix and session-switching evidence remain bound to D-527; this installation did not
repeat them or the earlier complete package checks.

D-528 makes the default test gates attribute processes and scratch directories to their own invocation. The final
candidate passed TeamAgents' 855-test gate and the supervisor's 19 independent registry checks. The merged source
also passed the 26-tool AArch64 GNU/QEMU target. Registry loss, malformed bindings and invalid markers fail
visibly; peer registries are retained regardless of age. Registry files can accumulate across runs. Unknown
ownership never authorizes a signal. See [ACCEPTANCE.md](ACCEPTANCE.md) for receipt paths and evidence boundaries.

The follow-up TeamAgents work completed successfully; the original field-debug operation remains
`OUTCOME_UNKNOWN` and was not rewritten or replayed. The installed CLI retains the accepted Unicode search fix.
The last complete package baseline remains `d96d7c8a`. CI at `a4ce2a2e` passed all four jobs in
[run 44000392754](https://github.com/seek-hope/TeamAgents/actions/runs/44000392754); that predates D-528.
The older scheduled updater-readiness failure in run 43868995516 remains a separate diagnosis obligation.
Current local formal replay retains its input-content binding in
[verification/REPORT.md](../verification/REPORT.md).

- **857 Rust tests**: core 198, engine 538, TUI 121.
- **63 positive TLA configurations in 60 traversals**, **323 controls**, **79 witnesses**, **9 Kani harnesses**.
- Catalogue: **387 configurations across 47 modules**, including an incomplete wide configuration.
- Current D-527 acceptance covers 23 isolated CLI invocations and seven PTY key/editor checks before and after
  installation, plus the source PTY matrix and installed session-switching probe. No real model was called.
- The package-baseline 18 offline probes, scripted profile/retry/tool/fetch and separate live Fetch API check
  remain bound to that earlier package; the interface installation does not renew those results.
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
- CLI validation rejects unrelated options and conflicting actions before session lookup or mutation. Scoped
  help and validation share `engine/src/interface.rs`; slash completion/help/dispatch share command metadata.
  Explicit permission overrides reach daemon boot, and JSON mutation options return structured receipts.
  Confirmations and mode changes require deliberate key presses with their documented modifiers; approval
  refresh follows the selected identity. `/edit [TEXT]` is explicit, while Ctrl+E edits the retained draft.
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
| CLI options and TUI action boundaries | `engine/tests/interface.rs`, `tui/tests/interface.rs`, CLI parser delimiter tests |
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

The original field-debug run retains one `OUTCOME_UNKNOWN` operation without quiescence evidence; the completion
refusal is correct. D-528 fixes the verification tool's ownership error that could kill a concurrent session's
runner, but does not establish that historical command's outcome. Do not infer success from missing processes or
bypass completion guards. The scheduled CI updater-readiness failure also needs diagnosis.

Specific runtime gaps include cross-provider switching and ambiguous wire identities, provider-side fetch
redirect/rebinding enforcement, session startup/filesystem races and full-backlog connection deadlines.
Wide exhaustive completion, Rust/SQLite refinement, whole-system composition and Policy B source-bound
bidirectional fair-trace equivalence remain open. Fair scheduling, escaped/remote effects and real power loss
remain environmental obligations. Retention policy does not authorize expiry of referenced output bytes.
