# Daily coding readiness and candidate work

Updated **2026-10-29** for D-489. This page owns the remaining candidate queue. Current behavior is in
[USER-GUIDE.md](USER-GUIDE.md), evidence in [ACCEPTANCE.md](ACCEPTANCE.md), and confirmed decisions in
[DECISIONS.md](DECISIONS.md). A queue item is not implementation, model-spend or publication authorization.

TeamAgents supports controlled daily coding on Linux with configured models and development tools. Current
evidence does not establish equal task success, lower cost or less intervention than Pi, nor a general sole-agent
replacement. The public v0.2.0 archive is older than current source.

## Implemented scope

| Area | Current behavior | Evidence boundary |
|---|---|---|
| Work continuity (D-487 / DD-11) | Shared behavior emphasizes user intent, initiative and excellent results. Goals persist across turns, compaction and restart; continuation is bounded; questions, attention, stops and completion are distinct. Subsequent work is admitted internally with its costs and limits | Scripted providers, SQLite/fault tests and recorded process groups; no measured long-task success rate. Heuristics can block legitimate repetition |
| Repository setup | TeamAgents reads repository-local configuration for trusted canonical roots and discovers instruction files; user-only policy stays at user level | Configuration tests and isolated probes; dependency diagnosis is not a complete build or Skill-workflow acceptance |
| Diagnosis and inputs (DD-01 / DD-04) | Read-only diagnosis, explicit recovery, instruction/source observations, immutable queued-input revisions and delivery receipts | Observation does not prove execution or completion; a config change is not a hot reload |
| Editing and tools (DD-05) | Atomic disjoint edits, bounded read parallelism, file/Shell/Skill tools, provider-neutral search, MCP and codemode | Host execution is not confinement. Complete cross-language/Skill workflows need broader evidence |
| Conversation and models (D-489 / DD-06) | Grok-style Leader conversation with elevated prompts, grouped activity, live status, compact model chrome and `@` file references; shared message/queue editing and complete answers | Scripted TestBackend/PTY checks, not a usability benchmark or every terminal/provider |
| Collaboration and recovery | Scoped task grants, independent members, worktrees, durable operations, runner recovery and declared-workspace takeover checks | Task cancellation is not process stop. Unknown effects are not replayed; remote/escaped effects are outside the local group observer |
| Verification and accounting | User-required checks, scoped completion evidence, known/unknown usage, native windows and cache telemetry | Check quality and semantic completeness remain distinct. Reviewer usage is still not charged to the goal |

DD-11 was approved and implemented as D-487; it is no longer an unapproved proposal. Its dated
[source study](../review/goal-autonomy-2026-10-28/REPORT.md) explains the upstream design inputs and their limits.
Local validation and its limits are recorded in ACCEPTANCE. D-461, D-467, D-469 and D-473/D-476 implement the
other selected DD items above. Their remaining evidence limits do not authorize arbitrary new features.

## Remaining candidates

Priorities express daily-development impact, not delivery promises. P0 blocks a general sole-agent
recommendation; P1 addresses recurring friction; P2 depends on the user's actual workflow. Each item needs a
bounded agreement. Paid trials, external-service changes and publication need separate authorization.

| ID / priority | Candidate | Required evidence or decision |
|---|---|---|
| DD-02 / P0 | Complete host development and Skill workflows | Real Rust, Python and Node edit/build/test tasks; relative Skill files/scripts; missing tools and capability revocation |
| DD-03 / P0, postponed | Matched Pi comparison on held-out coding tasks | Frozen versions/tasks, same model/native window/effort/authority/budget, repeated paired trials, hidden checks, honest outcomes, interventions and time; priced cost only if authorized |
| DD-07 / P1 | Complete model accounting and approval review | Every model-call class accounted for, unknown usage retained, independent adversarial review; dated price sources where authorized |
| DD-08 / P1 | Wider provider, image, MCP OAuth and terminal acceptance | Isolated real round trips with denial, revocation, restart and cleanup; the user's selected shell, terminal, multiplexer and SSH combinations |
| DD-09 / P2 | Additional integration coverage | Versioned contracts and runnable examples for a specifically needed auth, resource, SDK/client or extension interface |
| DD-10 / P2 | Distribution freshness | Exact authorized artifacts, clean-host install/update/rollback, retained config and source/artifact identity; documentation work does not authorize a release |

Artifact lifetime versus history retention (D-438) and the interpretation of Q16 collaboration (D-264) remain
policy questions. Ambient capability inheritance is excluded. Do not add a framework, larger TUI, marketplace,
LSP or automatic indexing merely to resemble another product.

## Comparison evidence

| Evidence | What it establishes |
|---|---|
| Pi 1.3.0 source/docs inspection, 2026-10-22 | Historical inspection only; the removed feature-by-feature table is available in Git history and is not a current product specification |
| D-485 follow-up, 2026-10-28 | Pi 1.4.0 was started offline; fixed Codex source `13b0217567d3` informed conversation UX. Codex was not executed, and no paired model trial ran |
| [D-489 TUI study](../review/tui-reference-2026-10-29/REPORT.md) | Pinned Codex, Grok Build and herdr implementations/screenshots; Grok selected for the closest single-conversation fit. Upstream source inspection is not an executed usability comparison |
| [D-450 comparison](../review/parity-2026-10-08/REPORT.md) | Earlier pinned versions and scoped presentation evidence |
| [D-451 maturity report](../review/maturity-2026-10-11/REPORT.md) | Immutable local workflow and installation receipts, valid for their pinned source |
| [Capability ledger](CAPABILITIES.md) and [benchmark ledger](../review/benchmark/README.md) | Historical real-service/coding results with their own models, sources and limits |
| Current local checks | `make check`, `make pty` and the formal targets; current results belong in ACCEPTANCE and the formal report |

Keep the Rust control plane, authoritative SQLite database, daemon/client boundary, minimal conversation and
scriptable management. User instruction and intent govern exploration. Existing authorization persists, while
new authority, replay of unknown effects and unselected policy changes require explicit decisions.
