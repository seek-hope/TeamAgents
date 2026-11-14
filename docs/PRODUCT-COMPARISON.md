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
- 777 Rust tests, 65 positive TLA configurations in 62 traversals, 327 controls, 79 witnesses and 9 Kani
  harnesses. PTY covers five terminal profiles and both forced graphics modes, plus `make probe-offline` (18/18)
  and `make probe-models` (23/23).

## Open work

| Priority | Open item | Required next evidence or decision |
|---|---|---|
| P0 | Matched Pi / alternative-harness comparison | An 8-run paired comparison (`deepseek-flash`, 1M native window, `high` effort) passed 8/8 hidden checks with 0 false claims on both TeamAgents ($0.0775, 229s) and Pi ($0.0161, 60s); broader long-horizon and multi-team task suites remain open |
| P1 | Reviewer and model judgement quality | Hardened `hard_deny` + untrusted-data framing passed 14/14 adversarial/injection cases on `deepseek-flash`; multi-model/SWE-scale adversarial coverage and config-driven USD price rates remain open |
| P1 | Provider and environment acceptance | All 4 wire protocols (`deepseek`, `chat/completions`, `anthropic`, `responses`), heterogeneous `deepseek`+`anthropic`, search/fetch, `deepseek-flash` images, loopback SSH and Alacritty passed locally; external OAuth services, public hosts and broader terminal/OS matrices remain open |
| P1 | Exploration-tree usefulness | Verified multi-branch abandonment, focus, zero dead-end re-reads on resume, and cold-tree 1M compaction on `deepseek-flash`; larger multi-session empirical lostness studies remain open |
| P2 | Integration contracts | Specifically needed versioned MCP auth/SDK/client/extension interfaces and runnable examples; resource list/read alone does not close this |
| P2 | Distribution acceptance | Local `make release-rehearsal` and isolated clean-host `install.sh` + live upgrade + broken-daemon rollback passed; genuinely clean multi-machine host acceptance remains open |
| P2 | Formal completion | Wide exhaustive search, full Rust/SQLite refinement, whole-system composition and Policy B source-bound bidirectional fair-trace equivalence |
| P2 | Repository configuration | reads repository-local configuration only through explicit project trust; permissions, hooks, checks, limits, retention and update policy remain user-only |
| P2 | Environmental semantics | Scheduler/provider fairness, filesystem races, escaped descendants, remote effects and real power-loss behavior |
| P2 | Retention and integration policy | Artifact lifetime versus history retention, broader dependency ecosystems and general long-horizon outcomes |

Paid trials, external-service changes, installation/runtime replacement and public release are separate operations
and are not implied by this queue.
