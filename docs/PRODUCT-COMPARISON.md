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
tokens. This measures local construction only; the older eight-run paired comparison predates this optimization.
The later repository cohorts below use the delivered runtime containing it.

| Priority | Open item | Required next evidence or decision |
|---|---|---|
| P0 | Matched Pi / alternative-harness comparison | Older trials and two infrastructure-confounded cohorts remain separate. With matched Cargo configuration, Pi passed 2/2 hidden suites, single Leaders 0/2, and one-member teams 1/2; both teams hit the deadline and both single Leaders incorrectly reported success. Broader tasks and reliable team completion remain open |
| P1 | Reviewer and model judgement quality | DeepSeek V4: 34/34 on reused fixed cases after correcting literal read-only search handling; Claude V3: 24/34 correct labels, with four parser failures and five 402 quota failures. No dangerous allow observed among completed cases; broader quality remains open |
| P1 | Provider and environment acceptance | All 4 wire protocols (`deepseek`, `chat/completions`, `anthropic`, `responses`), heterogeneous `deepseek`+`anthropic`, search/fetch, `deepseek-flash` images, loopback/public SSH reconnect and Alacritty passed locally; external OAuth services, clean hosts and broader terminal/OS matrices remain open |
| P1 | Exploration-tree usefulness | Verified multi-branch abandonment, focus, zero dead-end re-reads on resume, and cold-tree 1M compaction on `deepseek-flash`; larger multi-session empirical lostness studies remain open |
| P2 | Integration contracts | MCP revision `2025-06-18`, resource-only and paged catalogues, string IDs and local OAuth refresh pass; external OAuth, SDK/client/extension matrices and resumable push streams remain open |
| P2 | Distribution acceptance | Local `make release-rehearsal`, isolated existing-Debian install, managed upgrade, explicit rollback and injected failed-startup rollback passed; a genuinely clean multi-machine host matrix remains open |
| P2 | Formal completion | Wide exhaustive search, full Rust/SQLite refinement, whole-system composition and Policy B source-bound bidirectional fair-trace equivalence |
| P2 | Environmental semantics | Scheduler/provider fairness, filesystem races, escaped descendants, remote effects and real power-loss behavior |
| P2 | Retention and integration policy | Artifact lifetime versus history retention, broader dependency ecosystems and general long-horizon outcomes |

## Repository comparison follow-up (2026-11-15)

The task repairs version parsing in the real repository snapshot `9b012df9`, exported without future solution
commits. Four hidden tests were frozen before calls; the original helper failed and the reference passed.
Pi 1.5.2, a single TeamAgents Leader, and a Leader with exactly one member each received two independent runs.
All used DeepSeek Flash, its native 1,048,576-token window, high reasoning and an 8,192-token output limit.
Each run had 600 seconds, at most 100 requests and a conservative three-million-token reservation budget.
Prices were unavailable. The TeamAgents executable is the accepted archive identified in ACCEPTANCE.md.

The first cohort completed all six runs:

| Run | Hidden checks | Client outcome | Upstream calls | Known total tokens |
|---|---|---|---|---|
| Single Leader 1 | 3/4 | Failed: local reservation cap | 46 | 1,955,105 |
| Pi 1 | 4/4 | Final completion marker | 29 | 960,595 |
| One-member team 1 | 3/4 | Client deadline | 54 | 1,789,465 |
| Pi 2 | 4/4 | Final completion marker | 25 | 780,456 |
| One-member team 2 | 3/4 | Failed: local reservation cap | 50 | 1,990,503 |
| Single Leader 2 | 3/4 | Client deadline | 40 | 1,370,883 subtotal; one request unknown |

The four failed patches accepted bare JSON number `1.0` as a legacy version. Hidden-check success alone does
not establish that every agent-authored test passed or every task constraint was met. The first Leader also
changed three inventory documents; other patches changed only the updater source. Both teams created exactly
one member. No incorrect terminal completion claim was observed against the hidden checks.

This cohort had an environment confound: TeamAgents intentionally filtered the harness Cargo target variables
from Shell, while Pi inherited them. The clients therefore used different build caches. Preserve correctness,
usage and failure observations, but do not compare its elapsed times as matched performance. Local reservation
refusal is distinct from provider quota exhaustion. Unknown usage and prices do not mean zero cost.

The second cohort used tracked project Cargo configuration and verified the actual Shell/Bash target paths
without upstream calls. Its first Leader run stopped after 360.76 seconds when the 16 GiB temporary filesystem
filled. The runtime reported PARKED/storage-full. Runner shutdown refused one unsettled journal and one missing
journal, so the harness aborted before hidden grading and did not start the other five runs. Subsequent scans
found no scoped processes or recorded child process groups. The journals remain unchanged; process absence
does not reconstruct lost results. Only completed build caches were removed.

The third cohort completed all six runs with a shared Cargo target on the larger disk, four build jobs and
incremental compilation disabled for both clients. Actual Shell/Bash cache and environment checks passed
before calls. The harness required 2 GiB available for state and 32 GiB for build output before runs and each
upstream dispatch; no resource or local-budget refusal occurred. Model, task, checks, native window, order and
budgets were unchanged. All recorded upstream requests returned usage; prices remain unavailable.

| Run | Hidden checks | Client outcome | Seconds | Upstream calls | Total tokens |
|---|---|---|---|---|---|
| Single Leader 1 | 3/4 | Incorrect success report | 294.19 | 38 | 1,155,640 |
| Pi 1 | 4/4 | Final completion marker | 71.26 | 13 | 285,756 |
| One-member team 1 | 3/4 | Client deadline | 600.28 | 56 | 1,744,468 |
| Pi 2 | 4/4 | Final completion marker | 113.54 | 23 | 695,341 |
| One-member team 2 | 4/4 | Client deadline | 600.44 | 54 | 1,759,067 |
| Single Leader 2 | 3/4 | Incorrect success report | 467.00 | 37 | 1,190,063 |

All three failing patches accepted bare JSON number `1.0`. Both single Leaders returned `completed` with
work status `SUCCEEDED`, despite failing this check; their final summaries also omitted the required literal
completion marker. Analysis counts these as two incorrect completion claims and tracks marker compliance
separately. A marker-only analysis would miss them. The original cohort's zero incorrect-claim count is unchanged
under this rule. The second team produced a passing patch but did not finish its task within the deadline;
artifact correctness and completed work are separate outcomes. Both teams created exactly one member, and
all six patches changed only the updater source with no untracked files.

After all execution stopped, independent offline runs of the updater library and integration tests passed
for all twelve patches from the first and third cohorts: 24 commands, including all 83 agent-added tests.
Patch hashes remained unchanged. These checks confirm those authored tests pass; they do not repair the
hidden-check failures or turn deadline outcomes into success. All third-cohort daemon/runner stops succeeded
without refusals, and a scoped process/cwd/command-line plus recorded PID/process-group scan found no survivors.
This scan does not establish that arbitrary escaped descendants or remote effects stopped.

Frozen inputs, manifests, streams, patch/status receipts and analysis are retained locally under
`tools/tmp/repository-paired-eval/`; interrupted-cohort receipts are under `tools/tmp/repository-paired-eval-v2/`,
and the matched cohort is under `tools/tmp/repository-paired-eval-v3/`. Independent test receipts are under
`tools/tmp/independent-updater-checks/`. These ignored local artifacts are not shipped source-gate dependencies. The older four-task/two-round trial
remains a separate observation: both clients passed 8/8, with recorded totals of 229.15 seconds/556,371 tokens
for TeamAgents and 59.99 seconds/81,229 tokens for Pi. Its historical cost estimates were USD 0.077531 and USD 0.016141.
One known regression, two repetitions per arm, fixed execution order and pilot infrastructure failures cannot
establish general coding quality, team superiority or broad performance.

Paid trials, external-service changes, installation/runtime replacement and public release are separate operations
and are not implied by this queue.
