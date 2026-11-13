# Current capabilities and evidence

This matrix records current implementation and the kind of evidence available. It does not turn a local test or
finite model into full acceptance. See [ACCEPTANCE.md](ACCEPTANCE.md) for scenario coverage and open obligations.

| Capability | Current implementation and evidence | Current limit |
|---|---|---|
| Durable state and migrations | `core/src/v2/store.rs`; schema 9, WAL, `synchronous=FULL`, format refusal, explicit migration and conflict tests | No binary-only database rollback or arbitrary foreign-state adoption |
| Single-writer control plane | `Control::submit`; stable command replay, identity/revision assignment and atomic business/event writes | Concurrent SQLite/OS behavior and full refinement are not proved |
| Phase machine and recovery | `engine/src/v2/driver.rs`, supervisor and runner suites; known-result reuse and `OUTCOME_UNKNOWN` | Real crash timing and remote effects remain environmental |
| Work scopes and admission | Budget/deadline/admission/acceptance, internal successor work, bounded continuation and `core/src/v2/control/work_tests.rs` | Heuristics do not prove task impossibility or general success |
| Required checks and completion | `V2Checks`, required-check repair and completion evidence | A green command alone does not establish the whole deliverable |
| Automatic approval review | Strict assessment, current-user/policy context, matching completed review, self-update refusal and reservation/usage recovery | Lexical denial is not complete Shell parsing; live reviewer judgement and novel prompt-injection resistance are unmeasured |
| Grants and delegation | Parent narrowing/cascade, task-scoped non-delegable children, `V2Grants`, `V2Authority`, `V2Capabilities` | Arbitrary actor/command/identity composition is not exhaustively proved |
| Private history and messaging | `V2History`, `V2Inbox`, scoped messages and `read_history_is_user_or_self_only` | Host processes are not confined by the logical boundary |
| Workspace policies | `engine/src/workspace.rs`; shared, isolated and worktree lifecycle with dirty-work refusal and `V2Workspace` | No global lock or proof about escaped/remote effects |
| Native model requests | Provider adapters, native windows, per-member profiles, stable cache prefix and provider telemetry | Real-service availability, cost and full protocol behavior are not established by fakes |
| Provider framing and retries | `V2StreamBoundary`, bounded bodies, visible-text retry rules and stall tests | Provider scheduling/fairness and all wire variants are not proved |
| Compaction and history | `V2Compress`, long-context restart tests, originals/readback and lazy input revisions | Summary quality and full performance at latest source are not claimed |
| Session exploration tree | `core/src/v2/control/search_tree.rs`, `V2SearchTree`, CLI/daemon/export tests and ordered-pair witnesses | Tree usefulness, semantic completeness and reduced lostness are not measured |
| Ordered tree patches | Shared state conversion for single and two operations; atomic rejection, replay, 23 controls and 11 witnesses | Arbitrary operation sequences are not enumerated by TLA |
| Text and output safety | Unicode/control tests with normalized JSON and escaped terminal rendering, including bidi/isolate controls | No arbitrary byte-string parser proof; JSON uses valid Unicode strings |
| SQLite fault handling | Revision/head/event failure injection and savepoint tests, migration conflict rollback and random-walk invariants | Not a complete SQLite refinement or power-loss proof |
| File/Shell/Skill tools | `engine/src/tools.rs`, host receipts, relative-Skill and revocation checks; real Cargo/Python/Node workflows | Host execution is not confinement; broader dependency ecosystems remain open |
| Web search/fetch | Bound DeepSeek/Exa search and DeepSeek fetch, RRF/retry/cache and private-address tests | No fresh external-service acceptance is implied |
| MCP and codemode | stdio/HTTP fake services, resource list/read, ambiguity checks, restricted JavaScript and nested veto | Versioned external OAuth/SDK/client/extension coverage remains open |
| TUI conversation | Grok-style Leader view, Codex colors, grouped activity, conditional approvals and TestBackend/PTY checks | Not a usability benchmark or every terminal/provider combination |
| Editing and attachments | Shared message/queue editing, `@PATH`, immutable snapshots and original-input revisions | Symlinks and out-of-workspace files are refused; no arbitrary path acceptance |
| Markdown, code and media | Complete Markdown, bounded grammar highlighting, Mermaid/SVG/DOT/local images and source fallback | Remote resources and every diagram format are not promised |
| Sessions and export | Registry, history/search/fork/archive/restore/delete, consistent streaming export and `session_export_tests.rs` | Fork needs a stopped source and fresh execution evidence; no message-level branch execution |
| Installation and updates | Versioned install, update journal, health-check rollback, `tools/install_check.py` and `tools/release_rehearsal.py` | Clean-host two-version upgrade/rollback and current public-artifact acceptance remain open |
| Development tools | `tools/` contains current build, document, probe, formal and diagnostic utilities | `tools/tmp/` is ignored scratch, not committed evidence |

## Current evidence totals

- Rust tests: **773** (core 190, engine 478, TUI 105).
- Positive TLA inputs: **65 configurations in 62 traversals**.
- Negative controls: **327**, each required to refute its named claim.
- Normal-path witnesses: **79**, which demonstrate reachability rather than safety.
- Kani harnesses: **9** passed.
- Catalogue: **393 configurations across 48 modules**, including one separate wide configuration.

Search-tree evidence includes ordered pair transitions, 23 controls covering every non-`TypeOK` tree claim and
11 witnesses. `TypeOK` is tracked separately in [verification/COVERAGE.md](../verification/COVERAGE.md).

## Current open obligations

- Matched real-model task success, intervention and priced-cost comparisons.
- Live-model exploration-tree usefulness and reduced lostness.
- Wider provider/OAuth/public-host/graphical-terminal/clean-host acceptance.
- Wide exhaustive completion, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
  bidirectional fair-trace equivalence.
- Provider/scheduler fairness, filesystem races, escaped descendants and remote effects.
- Broader versioned integrations, artifact-retention policy and general long-horizon outcome measurement.
