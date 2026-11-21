# Current product comparison and open work

This is the candidate-work queue, not a record of past experiments. Implemented behavior is summarized in
[CAPABILITIES.md](CAPABILITIES.md); source-bound acceptance is in [ACCEPTANCE.md](ACCEPTANCE.md).

## Implemented and verified locally

- Durable work, scoped delegation, bounded recovery, required checks, known/unknown usage and optional frozen
  tariffs. Progress prose, passing artifacts and completed work remain distinct outcomes.
- Protocol 2 with bounded event/history pages, explicit retention resync and separate snapshot/delivery cursors.
- Host file/Shell tools, Web, Skills, MCP resources and restricted codemode. MCP revision `2025-06-18` is checked;
  resource-only bindings and bounded discovery/authentication retain normal authority checks.
- Atomic batch hash validation, target-ABI file flags, conservative endpoint-preservation and explicit-stop errors.
- Herdr-inspired team TUI with separate member history, tasks, activity and diagnostics. Navigation preserves
  Leader input ownership, drafts, attachments and approval ownership; hidden panels do not poll.
- Named-profile resolution before preparation/recovery, unchanged cached retries and public-host verification
  behind fake-IP DNS with UTF-8 byte caps.
- One revisioned session exploration tree with ordered atomic patches, Leader compaction and CLI/daemon/export
  projections. The tree is navigation, not authority or proof of success.
- The product reads repository-local configuration under explicit user trust; hooks, checks, permissions,
  limits, retention and update policy remain under user control.
- **874 Rust tests**, 63 positive TLA configurations in 60 traversals, 323 controls, 79 witnesses and 9 Kani
  harnesses. Current-host evidence covers five PTY profiles/both graphics modes, 18 offline probes, x86_64 musl
  installation and scripted profile/fetch checks; a separate live Fetch API check passed. AArch64 evidence is
  the 26-tool GNU user-emulation subset. None establishes general real-model task quality or native ARM acceptance.

## Open work

| Priority | Open item | Required next evidence or decision |
|---|---|---|
| P0 | Matched Pi / alternative-harness comparison | Held-out tasks, native context windows, matched models/budgets/build environments and independent grading; report correctness, completion, false success, intervention, latency and priced cost separately |
| P1 | Reliable team completion | Multi-branch tasks with bounded delegation, verification and recovery; a correct patch at deadline is not completed work |
| P1 | Reviewer judgement | Independent multi-model adversarial/benign cases, strict parsing and honest accounting of transport, quota and parser failures; reused development cases are not a holdout |
| P1 | Tree and interface usefulness | Correct/complete summaries and tree references, navigation usability and reduced lostness across representative sessions |
| P1 | Provider and environment acceptance | Current real-provider protocol/mixed-team/image coverage, broader proxy environments, provider-side destination/redirect checks, public SSH and graphical terminals |
| P2 | Request routing and local models | Explicit cross-provider switching and local-router list/load/unload have loopback coverage. Test automatic selection within user-authorized destinations, real router state and cost before claiming parity. |
| P2 | Integration contracts | External OAuth, versioned SDK/client/extension matrices, resumable streams and broader dependency ecosystems |
| P2 | Distribution acceptance | Current clean-host install/update/rollback, failed-startup recovery, schema transitions, physical hosts, broader distributions and native ARM/packages |
| P2 | Formal completion | Wide exhaustive completion, Rust/SQLite refinement, whole-system composition and Policy B source-bound bidirectional fair-trace equivalence |
| P2 | Environmental semantics | Session startup/check-to-use races, full-backlog connection deadlines, scheduler/provider fairness, escaped descendants, remote effects and real power loss |
| P2 | Retention policy | Artifact lifetime versus history retention without expiring referenced bytes absent an explicit policy |

Paid trials, external-service changes, installation/runtime replacement and public release are separate operations
and are not implied by this queue. Experiments need isolated state, bounded resources and source-bound receipts;
unknown usage or prices must never be reported as zero.
