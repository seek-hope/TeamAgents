# How this product compares with Codex CLI and Pi

Updated **2026-10-08** from fixed Codex source and installed Pi 1.1.0. This is a capability-surface
comparison, not evidence of equivalent task success, performance or reliability. The detailed twelve-dimension
matrix, inspected source receipts and ranked next-cycle acceptance plans are in the
[comparison report](../review/parity-2026-10-08/REPORT.md).

The previous 2026-09-30 comparison also discussed Hermes. That dated snapshot remains in Git history;
Hermes was not re-investigated in this Codex/Pi cycle and is not silently treated as current evidence.

## 1. Evidence and current differences

| Product | Reference | What was actually checked |
|---|---|---|
| Codex CLI | official commit `9b738582b13c2cdbeff54af0afd04c50c3e7ba09` | CLI/exec parsers, command popup, highlighting, config, compaction, subagent code and app-server documentation; no installed binary or live comparator trial |
| Pi | installed 1.1.0 | installed version, bundled CLI/session/security/MCP/extension documentation, distributed autocomplete and highlighting; no comparator model-success or crash trial |
| TeamAgents | baseline `a76722da`, D-450 candidate | repository source, before/after fixed fixtures, Rust tests, terminal probes and formal safety models; per-capability limits remain in [ACCEPTANCE.md](ACCEPTANCE.md) |

[The source manifest](../review/parity-2026-10-08/manifest.json) records dates, hashes, selected line receipts
and reproduction instructions. The report explicitly labels source-confirmed, documented, executed,
unknown and intentional differences. A missing file name does not establish a missing capability.

| Dimension | Codex / Pi reference | TeamAgents position |
|---|---|---|
| Input | Codex has a selectable command popup; Pi documents slash/path completion, file references, external editor and two queue types | D-450 completes the four local commands. File/image input convenience and editable queue visibility remain gaps |
| Code/media | Codex uses grammar highlighting with size/line guards; Pi uses explicit-language coloring with fallback | D-447/D-448 provide Markdown, Mermaid/SVG/DOT and local images. D-450 adds grammar coloring and schedules long-history previews from the viewport; no universal language/terminal parity claim |
| Sessions | Codex resume/fork/archive/delete; Pi picker, branch tree, fork/clone and export | Registry and CLI fork/search; optional TUI history/switch/delete. Message-level branches and local HTML export remain gaps |
| Tools | File/shell/tool providers, with different extension and review surfaces | Files/shell/web/MCP/skills/codemode pass through control-plane operations and receipts. Tool names alone do not compare coding quality |
| Headless/protocol | Codex exec JSON/schema plus app-server; Pi print, JSONL, RPC and SDK | Exec JSON/stream-JSON and local Unix protocol exist. No general embedding SDK |
| Config/instructions | Both support user/project configuration and instruction discovery | Validated CLI overrides (D-428); reads repository-local configuration at the nearest trusted project (D-434), plus instruction files. Project config requires user opt-in and cannot supply permissions, hooks, checks, retention or limits. CLI management is deliberate; the user TUI stays minimal |
| MCP/Skills/extensions | Both have MCP; Pi additionally exposes executable TypeScript extensions, packages and resource tools | MCP transports/OAuth, Skills and hooks exist. Resource browsing and programmable UI/provider extension APIs remain distinct gaps |
| Permissions/sandbox | Codex has configured sandbox/approval policy; Pi documents direct process permissions plus external isolation | Bound grants/approvals; bubblewrap, Docker and SSH. Micro-VM support is absent; Docker live evidence is less complete than bubblewrap evidence |
| Teams | Codex source has spawn/send/wait/resume/close subagent operations; Pi leaves subagents to packages | Persistent governed teams and shared/isolated/worktree policies. No measured cross-product collaboration-success ranking |
| Persistence/recovery | Codex stores threads; Pi stores resumable branches and compaction entries | SQLite state, independent daemon and recovery by durable location. Existing crash probes establish local behavior, not superiority over untested comparators |
| Context/usage | Both have compaction; Pi documents context/cost and branch summaries | Native context windows, compaction, usage/deadline/check accounting and explicit memory tools. Automatic cross-session injection remains outside the design |
| Acceptance/release | Codex has review; Pi offers extensible settlement hooks; both publish installation paths | Required checks gate settlement, independent verification is available. v0.2.0 delivers v2 (D-343), but its 2026-09-29 tag predates subsequent changes; this iteration is not released |

## 2. Implemented this cycle and remaining decisions

D-450 implements local completion, code highlighting and the media-cache repair. In the baseline fixture,
figures 16 through 19 of twenty never received previews. The same fixture now checks every visible placement,
including revisiting an evicted image; its receipt and timing conditions live in the report.

The next priorities are a release rehearsal/publication decision, input attachments, local session export and
navigation, editable queued input, stronger adversarial-renderer isolation, and live sandbox/provider evidence.
MCP resources, embedding interfaces, long-context observability and controlled collaboration evaluation remain
separate work. Each item in the report names evidence, user impact, dependencies and a next acceptance method.

Automatic cross-session context injection, distributed/multi-tenant execution and a plugin marketplace remain
explicit design exclusions; they need a user-confirmed design change. Explicit append-only memory and session
search already exist and are not the same as automatic injection. A release requires its own publication
authorization. No optional backlog item is silently bundled into this iteration.

## 3. Product choices preserved

TeamAgents retains a Rust-native control plane, one authoritative database per session, an independent daemon,
permission-scoped collaboration and required acceptance checks. The minimal Leader conversation and optional
session page remain the user interface; comprehensive developer controls stay in the CLI. Interrupting a model
request, detaching the TUI and deleting a session remain different operations.

These choices have tests and bounded formal evidence. They do not justify claiming that other products lack
formal methods, durability, multi-agent capability or equivalent outcomes without investigating those claims.

## 4. Limits and rechecking

The report and manifest are the current evidence. The older `review/codex_surface.py` and
`review/comparison_sources.py` are hand-run historical surface probes; their earlier dates/counts are not the
source of the refreshed table. Use the pinned capture script and manifest to reproduce this snapshot.
Uninspected package ecosystems, live vendor OAuth behavior, arbitrary graphical terminals and relative task
success remain unmeasured. Existing benchmark results are in [CAPABILITIES.md](CAPABILITIES.md) and were not
rerun, regraded or improved by this UI comparison.
