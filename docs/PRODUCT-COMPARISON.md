# Daily coding readiness and product comparison

Updated **2026-10-22** against TeamAgents **02be286d through D-474** and installed **Pi 1.3.0**. This is a
source/documentation comparison with local regression checks, not a cross-product coding benchmark.
The Codex comparison remains the dated [2026-10-08 report](../review/parity-2026-10-08/REPORT.md);
Codex was not re-investigated in this refresh.

**Assessment:** TeamAgents can perform real coding work and is a candidate for a controlled daily-driver
trial on Linux with configured models and development environments. The evidence does not yet establish
an interchangeable, sole replacement for Pi across general development workflows. Persistent execution,
scoped collaboration and required checks are useful capabilities; their existence does not establish
higher task success, lower cost or less operator intervention than another product.

This page owns the **candidate improvement queue**. [USER-GUIDE.md](USER-GUIDE.md) describes how the current
product works; [ACCEPTANCE.md](ACCEPTANCE.md) owns regression and acceptance evidence;
[DECISIONS.md](DECISIONS.md) owns confirmed behavior. D-461 delivers the user's selected DD-01 option A
(read-only diagnosis and explicit recovery), with automatic reminders excluded. Other candidates remain
unapproved implementation scope.

## 1. Current capability comparison

| Area | Pi 1.3.0 reference | TeamAgents at the inspected source | Boundary |
|---|---|---|---|
| Coding tools | File/shell tools, multiple disjoint edits in one edit call, callable tools in codemode | Read/search/write/edit with a disjoint `edits` batch (D-469), revision checks, atomic staged commit and persistent shell state | A batch is staged, not a crash journal; a failed edit rolls the batch back; writes stay ordered |
| Tool composition | Extensions document parallel tool execution and nested tool calls | Instances work concurrently; a batch of at least two read-only calls runs in parallel, writes and dispatch stay ordered (D-469); codemode composes bound MCP tools | Codemode's MCP host bridge is synchronous, not a parallel native-file/shell composition API |
| Input | File/image inputs, external editor, steering and follow-up queues | Explicit attachments, path completion, revision-checked pending input edit/cancel, an explicit `after = "turn"` follow-up queue (D-470); external editor through Ctrl+E and the /edit suffix | A follow-up is input, not a new goal; the queue does not implicitly send or reorder an in-flight turn |
| Intervention | Abort and queued-message controls | Model interruption, task cancellation, instance termination and UI detachment are distinct | Ctrl+C does not stop tools/delegated work; cancelling a task does not stop its running command |
| Sessions | Directory-grouped sessions, tree navigation, message-level fork and clone | Named sessions, search, history, session-level fork, local Markdown/JSON export | Fork refuses a source with a live daemon; no message-level branch tree or HTML export; no automatic per-directory session selection |
| Instructions | Automatic context-file discovery, including ancestor AGENTS files | Repository AGENTS.md files are discovered from the trusted boundary down to the cwd (D-467); the product also reads repository-local configuration under trust; user and project lists keep their precedence; untrusted roots contribute nothing | Loaded-source visibility and edits between turns remain unmeasured; discovery is only the file named AGENTS.md, no globs |
| Models | Interactive model/reasoning selection, provider-specific credentials and cloud/OAuth flows | Configured Chat Completions/DeepSeek, Responses and Anthropic adapters; mixed models across members; Ctrl+M or `/model` switches the Leader's model/effort at a READY/WAITING turn (D-473) | Only the Leader is offered; no full provider-auth catalog. MCP OAuth is not model-provider OAuth |
| Skills | Advertised skill descriptions/paths and access to supporting files | Registry search, SKILL.md and bounded relative support-file reading on the host (D-468, D-480) | No directory listing or binary streaming; a skill instruction grants no extra capability |
| MCP/extensions | Tools, resources, dynamic exposure, executable extensions and SDK | Stdio/HTTP tools, MCP OAuth, codemode, veto/notification hooks and local daemon protocol | No MCP resource browsing, general extension API or embedding SDK; hooks and MCP do not make Pi extensions compatible |
| Development environment | Tools use the Pi process's permissions unless externally isolated | Host Shell and local MCP; host tool discovery with doctor environment; controlled inherited environment and persistent member cwd/exports (D-480) | No built-in OS sandbox or per-call network isolation; full Skill dependency workflows still need evidence |
| Collaboration/recovery | Subagents are an ecosystem capability; sessions are resumable | Persistent governed teams, task-scoped capability delegation, worktrees, SQLite authority, independent daemon and durable receipts | D-461 diagnoses idle-unsettled work and offers explicit recovery without automatic reminders; no cross-product collaboration-success or recovery ranking was measured |
| Context/accounting | Context/cost display, compaction and provider cache usage | Native windows, compaction, readback, token budgets and cache hit/miss/write telemetry | No priced cost table; automatic approval-review calls are not charged to the goal yet |
| Verification/distribution | Published installation paths; extensible run-settlement hooks | Required checks gate successful settlement; local source and PTY gates pass | v0.2.0 is published (D-475); checks cannot settle a task whose model never calls finish; local tests do not establish service parity |

Current implementation anchors: [tool schemas](TOOLS.md), [tool executor](../engine/src/tools.rs),
[codemode](../engine/src/codemode.rs), [driver](../engine/src/v2/driver.rs),
[external editor](../tui/src/editor.rs), [session fork](../engine/src/v2/sessions.rs),
[configuration loader](../engine/src/config.rs), [providers](../engine/src/providers/mod.rs) and
[approval reviewer](../engine/src/v2/reviewer.rs). These are inspected code paths, not inferred absences
from file names alone. Native image attachments and view_image can reach supported models; codemode's
image helper still produces a marker rather than forwarding native image content.

## 2. Candidate improvements, in priority order

Priorities express daily-development impact, not delivery promises. **P0** blocks a general sole-driver
recommendation; **P1** reduces recurring friction; **P2** depends on the user's actual workflow. Each scope
needs agreement before implementation, and model spend, external services and publication need separate approval.

| ID / priority | Current gap and proposed direction | Acceptance to agree before work |
|---|---|---|
| DD-01 / P0 (A delivered) | D-461: user-confirmed read-only diagnosis and explicit recovery in CLI/TUI. RUNNING/idle, BLOCKED, approvals, registered waits and execution are distinguished; option B automatic model reminders was rejected | Projection, daemon restart, explicit send/cancel and TUI frame tests cover A. No observation-triggered model call, task transition or replay. Actual user intervention/cost benefit remains unmeasured |
| DD-02 / P0 (skill resources delivered) | Complete host development and Skill workflows. D-480 removes OS sandbox backends; D-468 support-file reads and D-479 host discovery remain | Real Rust, Python and Node edit/build/test workflows; a skill with relative references and a script outside the workspace; missing tools and capability revocation tested |
| DD-03 / P0 | Actual Pi replacement evidence: compare held-out daily coding tasks under matched conditions, not internal reference-loop ablations | Frozen product versions/tasks, same model/native window/effort/authority/budget, paired repeated trials; hidden tests, honest settlement, manual interventions, time and complete cost accounting. Report failures and uncertainty |
| DD-04 / P1 (discovery delivered) | Reliable repository instructions: automatic discovery with explicit scope, trust, precedence and loaded-source visibility. D-467 implements discovery from the trusted boundary to the cwd; D-479 adds live boot-selected sources, actual Leader prompt-assembly read results and scripted edit-between-turn regression; durable per-request provenance and broader worker/worktree acceptance remain | Parent/repository/subdirectory rules, worktrees, workers, missing/untrusted files and edits between turns; no implicit project permission or executable-hook escalation |
| DD-05 / P1 (multi-edit/read batches delivered) | Native tool composition and batch edits: expose bounded multi-edit operations and consider independent read parallelism/native tools in codemode. D-469 implements the disjoint `edits` array and parallel read-only batches; ordered per-instance tools and codemode composition remain | Multiple edits match the same original, stale revisions/overlap fail safely; permission, veto, cancellation and receipt paths stay intact. Measure latency/tokens before claiming a gain; do not promise crash-atomic multi-file changes without a journal |
| DD-06 / P1 (model/effort switch delivered) | High-frequency session controls: model/effort selection, explicit steering versus follow-up, project/session selection and message-level branching. D-473 adds Ctrl+M or `/model` for the Leader and D-470 adds an explicit `after = "turn"` follow-up queue; message-level branches and fork-while-live remain | No lost draft/input or implicit send; deterministic queued-input ordering and branch lineage; model/provider changes preserve valid context and accounting. A conversation branch does not roll back workspace files |
| DD-07 / P1 | Complete accounting and approval review: include reviewer usage in budget/deadline/unknown-usage handling; add context/compaction and per-member/request attribution | All model-call classes accounted for, missing provider usage remains unknown, prices have a source/date, cache read/write pricing is distinguished. Independent adversarial review of auto-review; substring denials are not a complete safety policy |
| DD-08 / P1 | Wider operational acceptance: actual selected provider/image/MCP OAuth paths and the user's terminal stack | Isolated real round trips with denial/revocation/restart and cleanup; Alacritty with zsh and chosen tmux/Zellij/SSH cases. PTY protocol checks alone do not establish graphical rendering or every platform |
| DD-09 / P2 | Provider and integration coverage: only the model auth, MCP resources, SDK/client or extension interfaces the user needs | Versioned contracts and runnable examples; provider authentication/refresh, resource URI/size/auth checks, lifecycle/cancellation and untrusted-extension boundaries. Existing Unix protocol remains an integration path |
| DD-10 / P2 | Distribution and documentation freshness: installable, matching binaries and accurate user-facing instructions | Exact authorized artifact, clean-host install/update/rollback, retained configuration and source-to-artifact identity. v0.2.0 is the first public release; future publication remains gated by explicit authorization (D-457, D-475) |

Remaining policy questions are not silently resolved by this ordering: artifact lifetime versus history
retention and conversation references (D-438), and whether the narrower measured collaboration-latency gain
satisfies Q16's success wording (D-264). Their history and other decision dependencies remain in
[ACCEPTANCE.md](ACCEPTANCE.md). D-455 closes the former blanket per-worker manual-shell-grant question:
opt-in, attenuated task-scoped delegation exists; ambient inheritance still does not.

Not prerequisites for a personal Linux trial: a plugin marketplace, automatic cross-session injection,
distributed/multi-tenant execution, HTML sharing, every provider or every operating system. The first three
remain outside the confirmed design. LSP, repository indexing or a larger TUI should not be added merely
because they sound like parity features; first identify a measured workflow that needs them.

## 3. Preserve the product's contracts

Keep the Rust control plane, one authoritative SQLite database per session, independent daemon, minimal
Leader conversation and scriptable management. A model interruption, cancelling a task, terminating an
instance and detaching a UI remain different operations. Task-result waits and message waits are distinct;
BLOCKED is not a terminal task-wait result. No automatic replay of unknown effects, implicit capability
inheritance, project-supplied acceptance policy or credential exposure is proposed here.

Changing those semantics, instruction trust/discovery, branch lineage or extension authority requires a
user-confirmed design decision. This documentation refresh authorizes no such code change.

## 4. Evidence and reproduction

| Evidence | Source and limit |
|---|---|
| Current TeamAgents inspection | 02be286d through D-474; includes D-455/D-458/D-459, the external editor, AGENTS.md discovery, multi-edit/read batches, terminal-matrix PTY checks and the Ctrl+M model/effort menu; inspect the anchors above |
| Current local gates | On 2026-10-26, make check passed: core 147 / engine 424 / tui 87. make pty passed the five-profile terminal matrix and both forced graphics protocols (D-472/D-473). These are local regression results, not an independent acceptance or paid-model trial |
| Pi reference | Installed npm package @earendil-works/pi-coding-agent 1.3.0; bundled usage, sessions, configuration, providers, skills, MCP, extensions and SDK docs; distributed edit-tool declaration. Documentation/source only, not comparator execution |
| Older comparison | [D-450 report and manifest](../review/parity-2026-10-08/REPORT.md): Pi 1.1.0 and Codex commit 9b738582b13c2cdbeff54af0afd04c50c3e7ba09. Historical, not refreshed by this document |
| Deterministic workflows | [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md), pinned to its recorded source; fake model responses, actual local daemon/runner/files/PTY and an unpublished installation rehearsal |
| Real coding results | [Capability evidence](CAPABILITIES.md) and [benchmark ledger](../review/benchmark/README.md). Historical runs do not become measurements of this source or Pi by being linked here |

Recheck TeamAgents without model credentials:

```bash
git status --short
git show --stat 02be286d
make check
make pty
```

For the Pi comparison, inspect the exact installed 1.3.0 package, not latest online documentation.
The following SHA-256 values identify selected bundled documents relative to that package; no package or
credentials were copied into this repository. A different hash requires a fresh inspection.

| Bundled document | SHA-256 |
|---|---|
| docs/usage.md | 6dfe7446c81d601d5ebef5769523115229f802da364fe90fcdf96e322ae9fbcd |
| docs/sessions.md | 152d6489f61a6de9349c8ceaade95d5498fbc74f21771f41ba8e87ce0e5d8936 |
| docs/configuration.md | fc48d1ce7d885c5253222415fb7a4b46d75f80e6431eb924174657ff775099eb |
| docs/providers.md | bc8798e4cc3209e455e3dc50c6652ae73c057d40122a4a741d8b5b65a041c2f1 |
| docs/skills.md | 5b9805525b194474ef1fe5455a0917ef15ed7f710aa9bec2eee11879556469e5 |
| docs/mcp.md | 23b9cca35d1340e8adce6bed8b495da0ebb58931179a937db87d610a1b40b9bf |
| docs/extensions.md | a2b5ff0f95e7e470ed0846d7c4fbfc5781fe7e694b8e7f51ea08859f3ad7325d |
| docs/sdk.md | d9034ccbf2219a58712d8dd4fcdf47e687d77131e71dd5d119d8a71e9bd2a2e8 |

## 5. Proposed daily-driver trial

Start with held-out bug fixes, multi-file features, refactors and tests, then add mid-task redirection,
long-command cancellation, reconnect/recovery, Skills/MCP and multi-project work. Fix versions, hidden checks,
budgets and failure classifications before either product sees the tasks. Use fresh isolated sessions and
paired randomized order; retain every attempt, intervention and side-effect receipt.

The existing [A/B/C evaluation](../review/eval/README.md) compares a direct reference loop, a persistent
single instance and collaboration **inside TeamAgents**; it is not a Pi comparison. The D-451 report's
[unexecuted evaluation proposal](../review/maturity-2026-10-11/REPORT.md#unexecuted-real-model-evaluation-proposal)
is a starting method, not an approved task set, spend cap or completed result. Calibrate costs separately,
then obtain authorization for the selected models and full trial. Use native windows, including the
user-confirmed 1M DeepSeek Flash window; never shrink them to force compaction.

A useful primary outcome is tests passing with honest settlement and no unauthorized modification within
the agreed budget. Report manual intervention, wrong completion, duplicate/lost effects, elapsed time and
provider usage/cost alongside it. Choose the replacement threshold before running; do not infer parity
from one green repository, a synthetic recovery fixture, test counts or a historical best score.
