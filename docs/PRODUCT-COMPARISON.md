# Daily coding readiness and product comparison

TeamAgents source was refreshed through **D-480 (`7ae6bd5b`)** on **2026-10-26**. The Pi reference is the
installed **Pi 1.3.0** package inspected on 2026-10-22. This is a source/documentation comparison with local
TeamAgents regression checks, not a matched coding benchmark. The
[Codex comparison from 2026-10-08](../review/parity-2026-10-08/REPORT.md) remains historical.

D-485 follow-up (2026-10-28): Pi 1.4.0 startup/help and Codex source `13b0217567d3` informed the confirmed
conversation UX work. Compact messages/tool groups, responsive hints, shortcut discovery and shared editing
are implemented. This scoped TUI comparison does not refresh the broader Pi 1.3.0 table or establish equal
interaction quality; current regression evidence is in ACCEPTANCE.

**Assessment:** TeamAgents can perform real coding work and is suitable for controlled daily-driver trials on
Linux with configured models and development tools. The evidence does not establish it as an interchangeable,
sole replacement for Pi across general development workflows. Persistent execution, scoped collaboration and
required checks are implemented; their presence does not show higher task success, lower cost or less operator
intervention than another product.

This page owns the candidate-work queue. [USER-GUIDE.md](USER-GUIDE.md) describes current behavior;
[ACCEPTANCE.md](ACCEPTANCE.md) records regression and acceptance evidence; [DECISIONS.md](DECISIONS.md)
records confirmed behavior. A queue entry is not implementation, model-spend or publication authorization.

## 1. Current capability comparison

| Area | Pi 1.3.0 reference | TeamAgents at the inspected source | Boundary |
|---|---|---|---|
| Coding tools | File/Shell tools, multiple disjoint edits in one call, callable tools in codemode | Read/search/write/edit with disjoint `edits`, revision checks, staged writes and persistent Shell state | A batch is not a crash journal; failed edits roll back that batch, writes stay ordered |
| Tool composition | Parallel execution and nested tool calls through extensions | Concurrent instances, parallel read-only batches and MCP composition through codemode | Per-instance writes and dispatch stay ordered; codemode's MCP host bridge is synchronous |
| Input | Files/images, external editor, steering and follow-up queues | Explicit attachments, path completion, pending-input edit/cancel and `after = "turn"` follow-up | A follow-up is input, not a new goal or an implicit interruption |
| Intervention | Abort and queued-message controls | Model interruption, task cancellation, instance termination and UI detachment | Ctrl+C does not stop tools; task cancellation does not stop a running command |
| Sessions | Directory grouping, tree navigation, message-level fork and clone | Named sessions, search, history, session-level fork and local Markdown/JSON export | No message-level branch tree, live-source fork or automatic per-directory selection |
| Instructions | Automatic context-file discovery, including ancestor AGENTS files | Trusted-boundary-to-cwd `AGENTS.md` discovery, explicit instruction files and source observations; reads repository-local configuration under the same trust gate | D-479 covers Leader prompt reads and edits between turns; broader worker/worktree evidence remains |
| Models | Interactive model/reasoning selection, provider credentials and cloud/OAuth flows | Four protocol families, mixed members, Ctrl+M model/effort menu and per-profile `efforts` | No full provider-auth catalog; MCP OAuth is not model-provider OAuth |
| Skills | Advertised skill descriptions/paths and support-file access | Registry search, `SKILL.md` and bounded relative support-file reads on the host | No directory listing or binary streaming; Skill text grants no extra capability |
| MCP/extensions | Tools, resources, dynamic exposure, executable extensions and SDK | Stdio/HTTP tools, MCP OAuth, codemode, hooks and a local daemon protocol | No MCP resource browsing, general extension API or embedding SDK |
| Development environment | Tools inherit the Pi process's permissions unless externally isolated | Host Shell/MCP, host tool discovery, controlled environment and persistent cwd/exports | No built-in OS sandbox or per-call network isolation; full Skill dependency workflows need evidence |
| Collaboration/recovery | Ecosystem subagents and resumable sessions | Persistent teams, task-scoped grants, worktrees, SQLite state, independent daemon and runner journals | No cross-product collaboration-success or recovery ranking was measured |
| Context/accounting | Context/cost display, compaction and provider cache usage | Native windows, compaction, readback, token budgets and cache telemetry | No priced cost table; reviewer usage is not yet charged to the goal |
| Verification/distribution | Published installation paths and extensible settlement hooks | Required checks, local source/PTY gates and published v0.2.0 | Current source can differ from the archive; local tests do not establish service parity |

Implementation anchors: [tool schemas](TOOLS.md), [tool executor](../engine/src/tools.rs),
[codemode](../engine/src/codemode.rs), [driver](../engine/src/v2/driver.rs),
[external editor](../tui/src/editor.rs), [session fork](../engine/src/v2/sessions.rs),
[configuration loader](../engine/src/config.rs), [providers](../engine/src/providers/mod.rs) and
[approval reviewer](../engine/src/v2/reviewer.rs). Native attachments and `view_image` can reach supported
models; codemode's `image()` returns a marker rather than native image content.

## 2. Candidate improvements, in priority order

D-461/DD-01 (work diagnosis), D-467/D-479/DD-04 (instruction discovery and source observations),
D-469/DD-05 (multi-edit/read batches) and D-473/D-476/DD-06 (model/effort controls) are implemented. They are
not pending candidates; their remaining evidence limits stay in the table above and in ACCEPTANCE.

Priorities reflect daily-development impact, not delivery promises. **P0** blocks a general sole-driver
recommendation, **P1** reduces recurring friction and **P2** depends on the user's workflow. Each item needs
a bounded agreement before implementation. Paid models, external services and publication need separate
approval.

| ID / priority | Current gap and proposed direction | Acceptance to agree before work |
|---|---|---|
| DD-02 / P0 | Complete host development and Skill workflows | Real Rust, Python and Node edit/build/test workflows; relative Skill references and scripts outside the workspace; missing tools and capability revocation |
| DD-03 / P0, postponed | Matched Pi replacement evidence on held-out daily coding tasks | Frozen versions/tasks, same model/native window/effort/authority/budget, paired repeated trials, hidden tests, honest settlement, interventions and time. Provider usage; cost only if separately authorized |
| DD-07 / P1 | Complete usage accounting and approval review | All model-call classes accounted for, missing usage remains unknown, price sources/dates where authorized, cache pricing distinguished, independent adversarial review |
| DD-08 / P1 | Wider operational acceptance for selected providers, images, MCP OAuth and the user's terminal stack | Isolated real round trips with denial/revocation/restart and cleanup; Alacritty with zsh and selected tmux/Zellij/SSH cases |
| DD-09 / P2 | Provider and integration coverage only where the user's workflow needs it | Versioned contracts and runnable examples for the selected auth, MCP resources, SDK/client or extension interface |
| DD-10 / P2 | Distribution and documentation freshness | Exact authorized artifact, clean-host install/update/rollback, retained config and source-to-artifact identity. Documentation cleanup alone does not authorize a release |

Artifact lifetime versus history retention (D-438) and the interpretation of Q16's collaboration requirement
(D-264) remain policy questions. D-455 provides opt-in task-scoped delegation; ambient authority inheritance
is still excluded.

A personal Linux trial does not require a plugin marketplace, automatic cross-session injection,
distributed/multi-tenant execution, HTML sharing, every provider or every OS. Do not add LSP, repository
indexing or a larger TUI without a measured workflow that needs it.

## 3. Preserve the product's contracts

Keep the Rust control plane, one authoritative SQLite database per session, independent daemon, minimal
Leader conversation and scriptable management. Model interruption, task cancellation, instance termination
and UI detachment remain distinct. Task-result waits and message waits are distinct; `BLOCKED` is not a
terminal task-wait result.

Do not introduce replay of unknown effects, implicit capability inheritance, project-supplied acceptance
policy or credential exposure. Changes to these semantics, instruction trust/discovery, branch lineage or
extension authority need a user-confirmed design decision.

## 4. Evidence and reproduction

| Evidence | Source and limit |
|---|---|
| TeamAgents inspection | D-480 `7ae6bd5b`, including host execution, environment/input observations and the implemented DD items above |
| Local gates | On 2026-10-26, `make check` passed with core 147 / engine 415 / tui 87; `make pty` passed five profiles and both forced graphics protocols |
| Pi reference | Installed `@earendil-works/pi-coding-agent` 1.3.0 and its bundled docs/source; no comparator execution in this comparison |
| Older comparison | [D-450 report and manifest](../review/parity-2026-10-08/REPORT.md): Pi 1.1.0 and Codex `9b738582b13c2cdbeff54af0afd04c50c3e7ba09` |
| Deterministic workflows | [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md), pinned to its recorded source |
| Real coding results | [Capability evidence](CAPABILITIES.md) and [benchmark ledger](../review/benchmark/README.md), each with its own source/date boundary |

Recheck TeamAgents without model credentials:

```bash
git status --short
git show --stat 7ae6bd5b
make check
make pty
```

For the Pi comparison, inspect the installed 1.3.0 package rather than assuming latest online documentation
matches it. These SHA-256 values identify the bundled documents used for the comparison:

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

## 5. Postponed matched trial

If the Pi comparison is resumed, use held-out bug fixes, multi-file features, refactors and tests, followed by
mid-task redirection, long-command cancellation, reconnect/recovery, Skills/MCP and multi-project work. Freeze
versions, hidden checks, budgets and failure classifications before either product sees the tasks. Use fresh
sessions and paired randomized order; retain attempts, interventions and side-effect receipts.

The existing [A/B/C evaluation](../review/eval/README.md) compares three harnesses inside TeamAgents, not
TeamAgents with Pi. The D-451 report's
[evaluation proposal](../review/maturity-2026-10-11/REPORT.md#unexecuted-real-model-evaluation-proposal) is a
starting method, not an approved task set or completed result. Use native model windows, including the
user-confirmed 1M DeepSeek Flash window.

Define the primary outcome and replacement threshold before running. Report manual intervention, wrong
completion, duplicate/lost effects, elapsed time and provider usage. Add monetary cost only under a separately
approved scope. One green repository, a synthetic recovery fixture, test counts or a historical best score
does not establish parity.
