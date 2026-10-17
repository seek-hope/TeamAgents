# Development handoff

Prepared for a new development session on 2026-10-17. This document preserves the completed session's
handoff; it is not a new implementation plan or an authorization to execute the backlog. Update it after
closing a development cycle. The design, decision log and acceptance ledger remain authoritative.

## Start here

1. Read [AGENTS.md](../AGENTS.md), then this document. Inspect Git status and recent commits before editing;
   do not assume the recorded baseline is still HEAD.
2. Read [DESIGN.md](DESIGN.md) for Q1-Q19 and A01-A36, the latest entries in
   [DECISIONS.md](DECISIONS.md), and the baseline/known gaps in [ACCEPTANCE.md](ACCEPTANCE.md).
3. Use [DEVELOPMENT.md](DEVELOPMENT.md) for build, test, generated-reference and evidence maintenance.
   [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) and the dated reports it links describe possible follow-up
   work. Agree the next bounded objective with the user before treating a candidate as approved scope.

## Completed baseline

The last implementation commit is **6f8a7402d017031be2bb797160cb5139287b1ea4** on main. It was pushed to
origin/main and verified against the remote ref in the completed session. Later documentation-only commits
may follow it. There is no unfinished implementation task carried over from that session.

| Cycle | Delivered | Source anchor |
|---|---|---|
| D-450 | Completion, code highlighting and media-cache repair | ea43869d |
| D-451 | File/image inputs, pending-input revision/cancellation, complete local Markdown/JSON export, session search, bounded Rust display workers, five deterministic coding/recovery workflows and unpublished installation rehearsal | e9a0aa84, 44353fc4, a06a5150 |
| D-452 | Review fixes and measured display bounds; final strict input page budget repair | c77a691a, b3898af6, 6f8a7402 |

D-452's independent read-only audit accepted all six criteria. The final audit reran the full check target and
Git whitespace check with exit 0, including formatting, Clippy with warnings denied, all three crate suites,
test-count reconciliation and hygiene. Counts: **core 134 / engine 387 / tui 75**. This paragraph records the
completed session's audit result; it is not a portable signed audit receipt. The code and regressions below
remain independently rerunnable after deleting the conversation. The old goal was completed and cleared;
there is no goal ID to resume in a new session.

D-451's deterministic runtime, PTY, formal and installation evidence remains in
[the maturity report](../review/maturity-2026-10-11/REPORT.md) and its immutable manifest/receipts. Those runs
belong to their recorded sources. D-452 did not rerun every formal proof, installed artifact or terminal probe,
and does not renew real-service acceptance. Neither round establishes general maturity, Pi parity or improved
real-model task success. The published v0.2.0 artifact predates these source changes; no new release was made.

## D-452 behavior and regression entry points

- Inputs CLI defaults to human-readable output; the JSON flag preserves machine output. The real-daemon CLI
  regression is in engine/tests/cases/inputs.rs.
- Pending-input pages hold at most 100 rows and 4 MiB of stored UTF-8 payload bytes. The daemon reads lengths
  before loading/parsing payloads. The cursor names the last included sequence. An oversized first remaining
  input returns an error naming its id/revision for explicit edit or cancel; it is never skipped or truncated.
  This is a payload budget, not a bound on the entire encoded response including metadata.
- Failed attachment submission immediately attempts to abandon this round's STAGING artifacts and delete its
  blob/temp files. Cleanup is best effort under storage/filesystem failure; boot recovery remains the crash
  backstop. Concurrent attachment submissions queue on an async lock.
- Captured children lead their own process group. Capture error, cancellation and timeout kill that group;
  completed captures preserve intentional long-lived descendants such as a session daemon.
- Client slash commands do not enter input history. Only messages submitted to the Leader do.
- Display wall deadlines are 3 seconds for media and 6 seconds for cold code grammars; the CPU backstop is
  8 seconds. Plain source remains available. Userspace cannot guarantee kernel/filesystem liveness for the
  parent's reads, spawn or wait operations.

Relevant focused regressions (all part of the normal suites):

- engine/tests/cases/inputs.rs: pending_inputs_edit_cancel_and_restart_through_daemon_and_cli,
  inputs_pages_have_a_byte_budget_without_losing_or_repeating_rows,
  concurrent_attachment_submissions_queue_instead_of_failing_busy.
- engine/src/v2/attachments_tests.rs:
  a_failed_submission_abandons_staged_artifacts_and_removes_blobs_immediately.
- tui/src/subprocess.rs: timeout_kills_the_whole_process_group_including_grandchildren.
- tui/tests/attachments.rs: slash_commands_never_enter_input_history_but_messages_do.

## Verification and local environment

There is no workspace-root Cargo manifest. Use the make entry point or explicit crate manifests.
Do not run leak-guard gates alongside live probes: their process/scratch censuses can interfere.

```bash
git status --short --branch
git log -5 --oneline
make check
# Additional terminal coverage when the change needs it:
make pty
```

The previous independent-check sandbox mounted the repository and host read-only, provided writable /tmp,
and set HOME to /tmp. Its initial failures were environmental: rustup could not find the installed toolchain,
then Cargo could not write the repository target directory. The working host-specific form was:

```bash
env RUSTUP_HOME=/home/rimuru/.rustup CARGO_HOME=/home/rimuru/.cargo \
  CARGO_TARGET_DIR=/tmp/cargo-check-target make check
```

Use the actual installed toolchain/cache paths on another host. Do not override HOME or PATH to work around
this: sandbox-toolchain tests derive their mounts from the Rust/Cargo homes. Ensure sufficient writable build
space; the previous host's /tmp was a 16 GiB tmpfs. Do not delete warm target directories just to tidy the tree.

Read-only inspection on 2026-10-16 found /usr/bin/rustup 1.29.1, distro package 1.29.1-1.1 installed on
2026-10-15, and Rust 1.95.0 selected by rust-toolchain.toml. No toolchain update was performed by that work.
D-452 records the inspection commands. A Docker test reports an explicit skip when its daemon is unreachable;
a green local gate is not evidence of a live Docker backend or external provider run.

The D-451 verifier checks exact source hashes. Run its offline verification at the pinned D-451 tree
(a06a5150), using a detached worktree or temporary Git archive if needed. It is expected to reject a later
source tree. Never rewrite that manifest to make current code appear to have passed old runs. Do not run its
write mode as routine cleanup. New runtime evidence needs a new dated receipt set.

## Files and state to preserve

- Tracked source, docs, frozen evaluations and immutable receipts are the portable handoff. Temporary logs
  under /tmp or /var/tmp and ignored agent journals are not required to resume development.
- The root session.sqlite is tracked. Treat it as existing project data, not disposable scratch. Do not
  delete or migrate it without understanding its provenance and the user's requested operation.
- Ignored agent state, logs, TLC states and build outputs can exist beside the tracked tree. Do not use a broad
  Git clean, database deletion or process-kill command to prepare a new conversation.
- Preserve the user's existing TeamAgents configs, sessions and self-update installation, including the
  default state under ~/.local/state/teamagents/v2. Test with isolated HOME/XDG/state/workspace directories.
- Credentials stay in environment variables or machine stores; never copy them into this handoff or receipts.

## Next-cycle choices and standing constraints

The previous cycle is closed. Candidate directions include an external editor, richer session branching or
HTML export, MCP resource browsing, embedding interfaces, context/cost observability, broader terminal tests,
and live provider/sandbox evidence. Consult the comparison and acceptance documents for priorities and limits;
none is implicitly selected by this handoff. Paid-model experiments and release publication need their own
concrete scope and authorization. Preserve native model context windows in any real-model experiment.

Keep the Rust control plane, one authoritative SQLite database per session, independent daemon and minimal
Leader TUI with CLI management. Input targets the Leader; interrupt cancels its model request, not tools;
exit detaches; deletion stays identity-bound; approvals do not steal the composer; no implicit dispatch.
Automatic cross-session injection, distributed multi-tenant execution and a plugin marketplace remain outside
the confirmed design. Design deviations require user confirmation before implementation.

The user's terminal is Kitty with zsh. Use isolated terminal checks and preserve owned Kitty image cleanup.
Retain existing presentation limits and fallback behavior when extending UI code. Code/docs stay English,
except the Chinese README and explicitly frozen evidence. Keep generated references and their inventories in
sync instead of weakening hygiene checks to accommodate documentation changes.
