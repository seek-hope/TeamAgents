# Current product comparison and open work

This is the only candidate-work queue. It records what is not yet established by current source or evidence.
Implemented behavior is summarized in [CAPABILITIES.md](CAPABILITIES.md); acceptance evidence is in
[ACCEPTANCE.md](ACCEPTANCE.md).

## Implemented and verified locally

- Persistent work, scoped collaboration, recovery, required checks and usage accounting.
- Files, host Shell, Web tools, Skills, MCP resources and restricted codemode.
- One Leader TUI with sessions, editing, approvals, media and `/tree` access.
- One session-level exploration tree with revisioned atomic patches, compaction integration, CLI/daemon/export
  projections and ordered-pair verification.
- Current development and verification tools under `tools/`.
- 773 Rust tests, 65 positive TLA configurations in 62 traversals, 327 controls, 79 witnesses and 9 Kani
  harnesses. PTY covers five terminal profiles and both forced graphics modes.

## Open work

| Priority | Open item | Required next evidence or decision |
|---|---|---|
| P0 | Matched Pi / alternative-harness comparison | Frozen versions/tasks, same model/native window/effort/authority/budget, repeated paired trials, hidden checks, honest outcomes, interventions, duration and priced cost if separately authorized |
| P1 | Reviewer and model judgement quality | Independent adversarial cases, current-model results, prompt-injection resistance and explicit price semantics |
| P1 | Provider and environment acceptance | Real providers and native protocols, Search/Fetch, images, MCP OAuth, public hosts and selected graphical-terminal combinations |
| P1 | Exploration-tree usefulness | Evaluate whether real models produce accurate, complete trees and reduce user lostness; scripted transition checks alone are insufficient |
| P2 | Integration contracts | Specifically needed versioned MCP auth/SDK/client/extension interfaces and runnable examples; resource list/read alone does not close this |
| P2 | Distribution acceptance | Exact authorized artifacts on a genuinely clean host: install, upgrade, rollback, retained config and source/artifact identity |
| P2 | Formal completion | Wide exhaustive search, full Rust/SQLite refinement, whole-system composition and Policy B source-bound bidirectional fair-trace equivalence |
| P2 | Repository configuration | reads repository-local configuration only through explicit project trust; permissions, hooks, checks, limits, retention and update policy remain user-only |
| P2 | Environmental semantics | Scheduler/provider fairness, filesystem races, escaped descendants, remote effects and real power-loss behavior |
| P2 | Retention and integration policy | Artifact lifetime versus history retention, broader dependency ecosystems and general long-horizon outcomes |

Paid trials, external-service changes, installation/runtime replacement and public release are separate operations
and are not implied by this queue.
