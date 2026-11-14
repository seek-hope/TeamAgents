# Current product comparison and open work

This is the only candidate-work queue. It records what is not yet established by current source or evidence.
Implemented behavior is summarized in [CAPABILITIES.md](CAPABILITIES.md); acceptance evidence is in
[ACCEPTANCE.md](ACCEPTANCE.md).

## Implemented and verified locally

- Persistent work, scoped collaboration, recovery, required checks and usage accounting. Optional USD tariffs
  are frozen per request; missing prices/usage and inherited usage without receipts remain explicit.
- Protocol 2 has bounded event/history pages, explicit retention resync and separate delivery/snapshot cursors.
- Files, host Shell, Web tools, Skills, MCP resources and restricted codemode. MCP revision `2025-06-18` is
  checked for stdio/streamable HTTP, resource-only bindings are retained, and catalogue pagination is bounded.
- One Leader TUI with sessions, editing, approvals, media and `/tree` access.
- One session-level exploration tree with revisioned atomic patches, compaction integration, CLI/daemon/export
  projections and ordered-pair verification.
- The product reads repository-local configuration after explicit trust; user permissions, hooks, checks,
  limits, retention and update policy remain under user configuration. This is current behavior, not open work.
- Current development and verification tools under `tools/`.
- **814 Rust tests** (full local gate passed), 65 positive TLA configurations in
  62 traversals, 327 controls, 79 witnesses and 9 Kani harnesses. PTY covers five terminal profiles and both
  forced graphics modes, plus `make probe-offline` (18/18) and `make probe-models` (23/23).

## Open work

Local request-construction measurements for the same 250-step history fell from 211,281 to 122,151 microseconds
(medians, about 42%). Output stayed identical: 501 messages, 5,024,689 serialized bytes and 1,256,173 estimated
tokens. This measures local construction only; the paired live comparison below predates this optimization.

| Priority | Open item | Required next evidence or decision |
|---|---|---|
| P0 | Matched Pi / alternative-harness comparison | An 8-run paired comparison (`deepseek-flash`, 1M native window, `high` effort) passed 8/8 hidden checks with 0 false claims on both TeamAgents ($0.0775, 229s) and Pi ($0.0161, 60s); broader long-horizon and multi-team task suites remain open |
| P1 | Reviewer and model judgement quality | DeepSeek V4: 34/34 on reused fixed cases after correcting literal read-only search handling; Claude V3: 24/34 correct labels, with four parser failures and five 402 quota failures. No dangerous allow observed among completed cases; broader quality remains open |
| P1 | Provider and environment acceptance | All 4 wire protocols (`deepseek`, `chat/completions`, `anthropic`, `responses`), heterogeneous `deepseek`+`anthropic`, search/fetch, `deepseek-flash` images, loopback/public SSH reconnect and Alacritty passed locally; external OAuth services, clean hosts and broader terminal/OS matrices remain open |
| P1 | Exploration-tree usefulness | Verified multi-branch abandonment, focus, zero dead-end re-reads on resume, and cold-tree 1M compaction on `deepseek-flash`; larger multi-session empirical lostness studies remain open |
| P2 | Integration contracts | MCP revision `2025-06-18`, resource-only and paged catalogues, string IDs and local OAuth refresh pass; external OAuth, SDK/client/extension matrices and resumable push streams remain open |
| P2 | Distribution acceptance | Local `make release-rehearsal`, isolated existing-Debian install, managed upgrade, explicit rollback and injected failed-startup rollback passed; a genuinely clean multi-machine host matrix remains open |
| P2 | Formal completion | Wide exhaustive search, full Rust/SQLite refinement, whole-system composition and Policy B source-bound bidirectional fair-trace equivalence |
| P2 | Environmental semantics | Scheduler/provider fairness, filesystem races, escaped descendants, remote effects and real power-loss behavior |
| P2 | Retention and integration policy | Artifact lifetime versus history retention, broader dependency ecosystems and general long-horizon outcomes |

Paid trials, external-service changes, installation/runtime replacement and public release are separate operations
and are not implied by this queue.
