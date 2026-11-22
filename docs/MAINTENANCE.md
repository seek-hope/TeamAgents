# Maintenance team

This is the operating charter for maintaining TeamAgents. It describes durable roles, ownership, the evidence a
change needs and how the team escalates. It is not a status report: the current work queue is
[PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md), acceptance evidence is [ACCEPTANCE.md](ACCEPTANCE.md), and the
session's live instance list is `teamagents instances`.

The baseline rules stay in [DESIGN.md](DESIGN.md) and [DECISIONS.md](DECISIONS.md). This charter does not override
them; a maintenance convenience that contradicts the baseline is a design deviation and needs the confirmation
[AGENTS.md](../AGENTS.md) requires.

## 1. Roles

Roles are durable; the instances that fill them are per session. One instance may hold several roles in a small
session, and one role may be split when its scope grows.

| Role | Owns | Gates that prove it |
|---|---|---|
| Leader | Integration, triage, user decisions, final acceptance | `make check` on the integrated tree |
| Dependency security | The three lockfiles, `deny.toml`, advisory reachability | `make deny` |
| Formal verification | `verification/`, TLA+ configurations and witnesses, Kani inputs, the report fingerprint | `make verify-model-all`, `make verify-model-counterexamples`, `make verify-witnesses`, `make verify-kani` |
| Runtime and tests | `core/`, `engine/`, `tui/` behavior and their integration suites | `make test`, `make pty`, `make test-aarch64` |
| Documentation and contracts | `docs/`, generated references, citations and inventories | `make hygiene`, the generators in [DEVELOPMENT.md](DEVELOPMENT.md) |
| Release and installation | Archives, installation, update and rollback rehearsal | `make release-rehearsal` |

The Leader is the only role that hands work to the user and the only role that accepts a member's report. A member
report is a claim: the Leader re-runs the acceptance check before integrating it (D-393, D-395).

## 2. Intake and priority

| Priority | Meaning | Response |
|---|---|---|
| P0 | A required gate is red on `main`, or a security advisory affects shipped code | Fix or revert before any other change; the tree is not releasable until the gate is green |
| P1 | An open item in [PRODUCT-COMPARISON.md](PRODUCT-COMPARISON.md) with a missing acceptance receipt | Plan it with the evidence it needs, then run it |
| P2 | Cleanup, documentation accuracy, ergonomics | Batch it behind P0 and P1 |

Triage order for an unexpected failure: reproduce it, name the smallest command that shows it, decide whether the
product or the check is wrong, then fix the cause. Changing an assertion, a control or a threshold so a red gate
turns green is not a fix.

## 3. Evidence per change

| Change touches | Minimum evidence before handoff |
|---|---|
| Any tracked file | `make fmt-check` and `make hygiene`; the full `make check` before the Leader integrates |
| `core/`, `engine/`, `tui/` behavior | The crate's suite; `make test` for anything cross-crate |
| A lockfile or dependency | `make deny` plus the tests of the code that uses the dependency |
| `verification/` inputs | The three TLA targets; `make verify-kani` when its toolchain is installed; then the report fingerprint |
| Terminal or UI rendering | `make pty` |
| Release, installation or update | `make release-rehearsal` and `python3 tools/install_check.py` |

Evidence means the command, its exit status and the source revision. A skipped, timed-out or partially processed
check is not a pass. A witness shows reachability, not safety. Counts come from `cargo` lists through
`tools/test_counts.py` (D-519), never from a hand-edited total.

Formal inputs carry an extra rule: `verification/REPORT.md` binds the verified input paths and bytes with a
fingerprint. Re-run the targets the changed input affects, and update the fingerprint only afterwards, stating
which targets were re-run and which kept an earlier result because their inputs did not change. Regenerating the
fingerprint is never a verification run.

## 4. Definition of done

- The change is in the working tree, formatted, and every gate it affects has a recorded exit status.
- Claims that were not verified are listed as unverified, in the handoff and in the final report.
- No credential, token, session database or machine-specific log enters a tracked file.
- Commits, pushes, releases, installations and paid model runs happen only with explicit user authorization. A
  green local gate is not that authorization.
- The documentation states the behavior that exists. Superseded text is replaced in place; Git keeps history.

## 5. Coordination

- A shared workspace has one active writer per file set. Before taking over another writer's workspace, confirm
  that its processes stopped: cancelling a task does not prove that, and a killed parent does not prove that a
  child exited.
- Capability grants widen only with the user's approval; a member parked in `WAITING_CAPABILITY` is not working,
  and its task must not be reported as in progress. Narrow `task_result` grants are the normal return path.
- An unknown external outcome stays `OUTCOME_UNKNOWN` and is never replayed on a guess. A parked or blocked task
  is not success.
- Retry only while no visible text was emitted and the attempt budget remains; a protocol failure ends the attempt
  honestly.
- Budgets, deadlines and usage ceilings apply to members as they do to the Leader. Unknown usage is recorded as
  unknown, not as zero.

## 6. Cadence

| Cadence | Check |
|---|---|
| Every change | The row in section 3 that applies, plus the gate the change could break |
| Daily | The scheduled CI watchdog and the current red-gate list |
| Weekly | `make deny` against fresh advisories; the lockfiles and the pinned toolchain |
| Monthly | The full formal set, `make pty`, `make test-aarch64`, `make release-rehearsal` and an inventory regeneration |

A scheduled run that fails is triaged like a user report. A gate nobody runs is a claim nobody checked.

## 7. Escalation to the user

Ask the user, in one grouped question, before: widening a capability or a grant scope, terminating or replacing an
instance, starting a paid model run, publishing or pushing, installing or rolling back a release that replaces
running binaries, or changing a confirmed design rule. Independent work that does not depend on the answer
continues meanwhile.
