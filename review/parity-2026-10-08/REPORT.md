# Codex CLI / Pi comparison and first improvement cycle (2026-10-08)

This is a surface and behavior comparison, not a model capability benchmark. No paid model calls or
comparator task-success trials are part of this cycle. The approved implementation scope is local command
completion, fenced-code highlighting, and media scheduling in long history. The remaining gaps below are
not marked complete merely because this cycle finishes.

## Sources and reproduction

[manifest.json](manifest.json) records fixed versions, dates, full-source SHA-256 values, selected line
receipts, and replay instructions. [capture.py](capture.py) retrieves public sources and reads the installed
Pi package without accessing credentials. Raw downloads used for inspection are temporary at
`/var/tmp/teamagents-parity-sources`. The receipts are selective excerpts, not complete source snapshots.

- **Codex: source checked**, official commit `9b738582b13c2cdbeff54af0afd04c50c3e7ba09`, resolved on the date
  above. No Codex executable is installed on this host; no live Codex behavior or performance is claimed.
- **Pi: installed version checked**, 1.1.0. Its bundled documentation and distributed implementation take
  precedence over the older 0.99.2 skill reference. CLI/version checks are executed; documentation claims
  are not promoted to live model, sandbox or recovery evidence.
- **TeamAgents: source and fixture checked**, clean baseline `a76722da`, with the final implementation and
  verification recorded below. The fixture is [../../tui/examples/parity_probe.rs](../../tui/examples/parity_probe.rs).

```sh
cargo run --offline --locked --manifest-path tui/Cargo.toml --example parity_probe
python3 review/parity-2026-10-08/capture.py --out /tmp/parity-recapture
```

For a baseline rerun, extract the baseline's TUI with Git archive into a fresh temporary directory, copy
the same probe source into its examples directory, and run the example against that manifest. Use a separate
Cargo target directory; this does not replace the working tree. Compare fixture hashes before interpreting
the before/after values. Recapture references into a fresh directory, not over the original receipts.

Conda Python's CA store initially refused the GitHub connection. Curl using the host certificate store
retrieved all pinned sources with TLS verification enabled; no insecure transport override was used.

### Source key

Every path below has a corresponding manifest record with full-source hash and replay command. The C/P/T
labels used in the matrix identify that record family, rather than asserting equivalence of evidence classes.

| Key | Inspected source at the version above | Evidence class |
|---|---|---|
| C-cli | Codex CLI main and exec CLI parsers | Source: subcommands, flags, image arguments, structured output |
| C-ui | Codex command popup and syntax highlighting implementation | Source: candidate filtering/selection; syntect/two-face and parser input limits |
| C-config | Codex core configuration module and configuration documentation | Source/documentation: providers, projects, requirements, sandbox and approval choices |
| C-app | Codex app-server README | Documentation: thread storage/listing, interruption, resource reads and managed policy; experimental features stay experimental |
| C-team | Codex multi-agent handler and spawn implementation | Source: spawn/send/wait/resume/close, child role/model configuration and depth checks |
| C-context | Codex compaction implementation | Source: inline auto-compaction, replacement history and compaction hooks |
| P-cli | Pi bundled CLI, usage and RPC documentation | Documentation, plus installed CLI/version checks: print/JSON/RPC, queued input, files/images and command discovery |
| P-ui | Pi distributed autocomplete and theme implementation | Source: completion application and explicit-language code highlighting with fallback |
| P-session | Pi sessions and compaction documentation | Documentation: tree branches, fork/clone, summaries, storage and usage |
| P-config | Pi configuration and security documentation | Documentation: user/project resources, context-file discovery and trust boundaries |
| P-ext | Pi extensions, MCP, skills and packages documentation | Documentation: executable extensions, hooks, resources, OAuth and package distribution |
| T | TeamAgents baseline design, user guide, capabilities and actual TUI sources | Source plus local tests/probes; stronger claims remain limited by the acceptance ledger |

## Twelve-dimension comparison

| Dimension | Codex CLI | Pi 1.1.0 | TeamAgents and consequence |
|---|---|---|---|
| Interaction and input | Command popup filtering and selection, source checked (C-ui) | Slash discovery, path completion, file references, external editor, steering and follow-up, documented; autocomplete source checked (P-cli/P-ui) | Four local commands and a minimal Leader composer. This cycle adds completion without adding management pages. File references, external editor and a separate editable follow-up queue remain gaps |
| Code and media display | Language grammars and guardrails, source checked; no graphical-terminal run here (C-ui) | Explicit-language highlighting in theme code; inline image behavior is terminal-dependent (P-ui/P-cli) | Markdown, Mermaid/SVG/DOT, local images and Kitty/block paths already exist. This cycle adds token coloring and fixes media starvation. Broad terminal/image-protocol parity is unmeasured |
| Sessions | Resume/fork/archive/delete/agents commands and app-server thread APIs, source/documentation checked (C-cli/C-app) | Searchable picker, tree navigation, fork/clone and local exports, documented (P-session) | Named sessions, CLI fork/search, TUI paged history/switch/delete are implemented. No message-level branch tree or local HTML export; these are distinct from resuming a session |
| Tools | File/shell/review/apply and configurable tool providers are present in source; no task-quality comparison (C-cli/C-config) | Built-in file/shell/search tools, codemode and dynamic tool search, documented (P-cli/P-ext) | File/shell/web/MCP/skills and codemode use durable operation receipts. Tool-set breadth does not establish editing accuracy or coding success |
| Headless operation and protocol | Exec with JSON/schema output and app-server interfaces, source/documentation checked (C-cli/C-app) | Print, JSONL events, bidirectional RPC and TypeScript SDK, documented (P-cli) | Exec JSON/stream-JSON plus Unix-socket daemon protocol. No general embedding SDK; calling Pi interactive-only was wrong |
| Configuration and project instructions | CLI overrides, provider/project config and policy requirements, source checked (C-cli/C-config) | User/project settings, reload, prompt resources and layered context files, documented (P-config/P-cli) | Validated CLI overrides, nearest trusted project config and instruction files exist (D-428/D-434). Management stays in CLI; no in-TUI model picker is an intentional Q6 difference |
| MCP, skills and extensions | MCP/plugin commands and app-server resource reads, source/documentation checked (C-cli/C-app) | Built-in MCP transports/OAuth/resources, Skills and executable TypeScript extension packages, documented (P-ext) | MCP transports/OAuth, Skills and pre-tool/notify hooks exist. MCP resource browsing and executable UI/provider extension APIs are not equivalent to having tool calls; wider ecosystem remains a gap, marketplace remains outside the design |
| Permissions and sandbox | Configured approval/sandbox choices and managed policy, source checked (C-config/C-app) | Direct process permissions by default; external containers/VMs or tool wrappers, documented. Project trust is explicitly not a sandbox (P-config) | Bound capabilities and explicit approvals; bubblewrap/Docker/SSH shell backends. SSH runs remotely and is not a stronger local isolation theorem. Docker live-service depth and a micro-VM backend remain separate gaps |
| Teams and collaboration | Explicit subagent operations and configurable child roles/models, source checked (C-team) | Core README deliberately leaves subagents to extensions/packages; no installed third-party package was benchmarked (P-ext) | Governed persistent teams with shared/isolated/worktree policies. Earlier claims that Codex only has one agent are false; relative collaboration success is unmeasured |
| Persistence and recovery | Stored threads, archive/delete and compaction replacement history, source/documentation checked (C-app/C-context) | Automatically saved branchable sessions and compaction entries, documented (P-session) | SQLite control transactions, independent daemon, durable receipts and unknown-outcome parking have existing local failure probes. No identical crash experiment was run against the comparators, so there is no comparative reliability ranking |
| Context and usage | Compaction and per-thread model/config settings, source checked (C-context/C-config) | Context/cost footer, automatic/manual compaction, retained branches and summary usage, documented (P-cli/P-session) | Native model windows, durable compaction, usage/deadline/check accounting and opt-in output/context ceilings exist. Explicit append-only memory exists (D-405/D-406); automatic cross-session injection remains outside the design. Benefit depends on the task |
| Acceptance and release | Review command, repository tests and installation instructions; no equivalent TeamAgents settlement contract or release test executed (C-cli) | Package checks/tests and extensible settlement hooks; no built-in independent acceptance guarantee inferred (P-ext) | User-defined checks gate settlement, independent verification is available and model/test evidence is maintained. v0.2.0 is the v2 product (D-343), but its 2026-09-29 tag predates later changes. Current-iteration installability must not be inferred from a local green test run |

The comparator source snapshot is newer than the old binary/help snapshot. No assertion depends on the
absence of a filename. Uninspected extension packages, provider-specific behavior, live MCP vendors and
relative task success are **unknown**, rather than counted as absent or inferior.

## Reproduced defects and implemented changes

**Completion.** The baseline Tab on /hel left the draft unchanged. The conditional menu now lists only
existing commands. Up/Down selects, Tab or Enter fills, Escape dismisses without changing the draft, and
submission follows on an explicit Enter. A fully typed command keeps its ordinary one-Enter behavior.
Reported repeated Enter key events do not submit a just-filled command. Approval focus and multiline or
non-command text retain their own controls. Plain terminals cannot distinguish repeated physical key presses
from identical bytes, so this does not claim hardware key-up semantics on every terminal.

**Highlighting.** The baseline Rust code body had one color. The recorded two-color count includes its
border prefix; it is not evidence of two grammar-token colors. The shared renderer now uses syntect 5.3.0
and two-face 0.5.2+bat-0.26.1 with an explicit language and a fixed dark theme. Tests verify Rust, Python,
JavaScript, TypeScript, JSON, TOML and Shell, plus the existing diff colors. Code remains plain for unknown
languages, incomplete fences, parser errors, more than 64 KiB/500 lines, or any line over 4096 bytes. Source
text/indentation is retained; terminal controls are filtered before parsing. Input limits are not a hard
CPU deadline inside one regex call; adversarial-input isolation remains a follow-up.

**Media starvation.** The baseline transcript visited every block from its beginning and protected every
encountered request from eviction. Sixteen early entries permanently excluded later entries, even when
scrolling. Layout now stores deferred media descriptors; only descriptors intersecting the actual viewport
request work. Lightweight height metadata follows the current document separately from the sixteen decoded
cache entries. Evicted images can reload without collapsing known layout height. The queue stays at eight
requests; stale-generation results cannot repopulate the current cache, but do wake retries when they free
queue space. This applies to live conversation, streaming and saved session history. Workspace confinement,
source toggling, clipping and the user's 64 KiB/500-line diagram limit remain in force.

### Same-fixture measurement

The original diagnostic [before.json](receipts/before.json) measured the baseline and identified missing
figures 16 through 19. The final fixture records **actual visible placements**, rather than inspecting
whether the eager transcript happened to retain every decoded image. That observation method was rerun
against an isolated baseline checkout as well as the candidate. Rapid sweeps show 16/20 before and 19/20
after: the candidate first image did not decode during its brief visible interval. These receipts remain in
the tree as before-visible and after-visible; this is not counted as complete coverage. The final paired
probe adds 24 individual visits (all twenty items, then 0/19/1/18 again), with the same two-second deadline
per visit on both versions. It observes actual visible image placement and records every visit. This
distinguishes incomplete work during a quick scroll from permanent denial after the first sixteen items.

Both runs use twenty distinct synthetic SVG diagrams, native Kitty documents in Ratatui TestBackend,
80 by 28 cells, two forty-step top-to-bottom sweeps and 800 key-plus-frame samples. They are local synchronous
application measurements, not end-to-end terminal latency, real-model output, or a graphical Kitty test.
No benchmark or leak-check process ran concurrently. The host exposed 20 logical CPUs; load averages are
recorded (before 7.88/10.52/10.62, after 8.97/10.62/10.64). This is one paired observation on a shared host,
not a statistical speed claim. Key-plus-frame timings retain the 800 sweep samples; wait times come from
the subsequent individual visits.

| Observation | Baseline a76722da | Candidate |
|---|---|---|
| Unique images shown within individual visit limits | 16/20; missing 16..19 | 20/20; none missing |
| Individual visits including eviction/revisit | 18/24; six expired at approximately 2 seconds | 24/24; maximum 218.55 ms |
| Key-plus-frame p50 / p95 / maximum | 1.63 / 2.35 / 6.35 ms | 2.05 / 3.20 / 5.66 ms |
| Tab on /hel | Draft stays /hel; no effect | Draft becomes /help; no effect |
| Rust row color count including border | 2 | 5 |
| Draft retained | yes | yes |

Raw results: [before-bounded.stdout](receipts/before-bounded.stdout) and
[after-bounded.stdout](receipts/after-bounded.stdout). Their JSON sidecars record command, working directory,
source hashes, time and exit status. The baseline sidecar additionally hashes the isolated execution tree;
its production inputs match the Git baseline and its probe is byte-identical to the candidate probe.

### Final verification receipts

The receipt runner writes separate stdout/stderr and actual exit status; it does not infer success from a
trailing shell command. The manifest indexes each recorded run and hashes its sidecar. Run
`python3 review/parity-2026-10-08/verify.py` to validate source and run receipts and the paired result.

| Command | Receipt name | Result |
|---|---|---|
| make check | check | exit 0, 161.677 s; core 130 / engine 376 / TUI 57; format, warnings-denied Clippy, tests and hygiene |
| make pty | pty | exit 0, 44.205 s; completion, draft/focus, Markdown and Kitty/block graphics paths with scripted service |
| python3 review/dogfood/tui_sessions.py | sessions | exit 0, 21.826 s; real daemons, paged history, failed/successful switch, drafts and confirmed deletion |
| python3 review/dogfood/tui_reconnect.py | reconnect | exit 0, 4.694 s; daemon loss/restart, 0.5 s reconnect, fresh state and retained draft |
| python3 review/dogfood/input_latency.py | input-latency | exit 0, 15.424 s; scripted service, 0.10 s single-key median (coarse sampling), 0.12 s ten-character burst |
| make verify-model-all | formal-final | exit 0, 282.831 s; 42 configurations |
| make verify-model-counterexamples | formal-controls | exit 0, 241.500 s; 125 intended refutations |
| make verify-kani | formal-kani | exit 0, 21.792 s; 7 harnesses |

Formal material was committed first as b4caf2f1 and 5848f8d2, then the formal report pinned 5848f8d2.
The earlier full positive run spanned a whitespace-only model declaration adjustment; formal-final reruns
all configurations with a fixed source snapshot. Product sources and tests are unchanged since the full
check; the measurement example subsequently gained bounded individual visits and was compiled/run on both
versions and checked again with all-target Clippy. Source hashes distinguish those stages. All service probes
used isolated temporary roots, and none called a real model or altered an existing user session.

### Verification boundary

Five new integration tests in the TUI cover completion effect ownership, 42 geometry combinations, seven
language grammars and saved history, safe fallback/control filtering, and visiting all twenty images then
revisiting evicted ones. Existing raster/path/graphics tests cover image formats and reference confinement.
The PTY smoke additionally verifies candidate display, Tab with no business effect, explicit submission
exactly once, cancellation with draft retention, code indentation, and the existing media protocol paths.

The presentation model checks completion-only editing, cancelled-draft preservation, visible-only media
admission, bounded cache reservations and stale-result rejection. Each property has an independently refuted
negative control. The positive model explores 25,545 distinct states. This is an abstract safety model,
not a Rust refinement proof, a proof of regex/graph-layout termination, or an eventual-preview theorem.
The fixed fixture and integration test supply the concrete eventual-preview evidence for the tested inputs.

## Ranked next-cycle backlog

Priorities describe impact on the intended coding workflow, not a feature count. None of these rows is
silently included in the current implementation or marked complete. Dependencies requiring a changed design,
a paid trial or publication must be resolved in the next approved specification.

| Priority / item | Evidence and user impact | Intentional difference? | Dependency and next acceptance method |
|---|---|---|---|
| P1: publishable current release | D-343 closed the older-generation gap; the v0.2.0 tag at 76926407 predates this iteration and its later runtime/TUI features | No | Release rehearsal, clean-machine install/doctor/exec/session smoke, then explicit publication authorization |
| P1: adversarial presentation work | Regex and graph layout execute in process with bounded inputs but no interruptible per-call deadline | No | Fixed adversarial fixtures, input-latency/memory measurements, isolate parser workers if needed; document supported limits |
| P1: file/image input convenience | P-cli documents file references, path completion and image paste; TeamAgents currently renders stored images but lacks that composer flow | No | Attachment identity/workspace/permission design; native image-part, unsupported-model and terminal paste tests |
| P1: local session export and navigation | P-session has tree/fork/export; TeamAgents has session-level fork/search and paged history | Partly: an optional browser must stay minimal | Design a CLI export and optional search/filter; verify stable pagination, complete tool/image references and no automatic upload |
| P1: explicit queue visibility/editing | P-cli distinguishes steering/follow-up and retrieval; TeamAgents queues at a control boundary but offers less editing visibility | No, while interruption semantics must stay explicit | Preserve durable input identity and exactly-once consumption; queued-draft edit/cancel and crash/reconnect probes |
| P1: real sandbox/provider evidence | Capability ledger records uneven live Docker/MCP OAuth evidence; comparator source cannot fill those gaps | No | Isolated real service fixtures and inspected receipts; never infer live compatibility from a mock or a green unit suite |
| P2: MCP resource surface | P-ext and C-app distinguish resources from tool calls; current tool binding support is not complete resource UX parity | No | Scoped list/read contracts, pagination, mixed text/image/binary results and OAuth failure probes |
| P2: embedding and extension interfaces | P-ext/P-cli provide SDK/RPC/custom renderers; TeamAgents has a Unix protocol, Skills and hooks | Partly: Rust-native core and no marketplace are intentional | Prioritize concrete integrations; protocol lifecycle/error/backpressure tests before introducing an SDK or stable extension ABI |
| P2: context observability and cost | Existing usage/compaction evidence does not measure every long-project workflow; input caps must retain native model windows | No | Preregistered tasks and native-window metadata, cost attribution and real receipt analysis with an approved pilot budget |
| P2: collaboration outcome evidence | Q16 has latency evidence but not a general end-to-end success advantage; C-team invalidates a single-agent-only comparison | No | Fixed tasks, same model/budget, complete paired ablations; retain null results and independently inspect graders |
| P2: arbitrary terminal portability | D-448 has Kitty and Unicode/block paths, not every graphics protocol/multiplexer | Partly: a supported terminal matrix is preferable to unsupported inference | Actual terminal-specific probes and screenshots; fallback remains readable and clean on exit |
| Design boundary: automatic cross-session injection, cloud/multi-tenant backends, marketplace | Existing DESIGN exclusions; explicit memory/search are already available and should not be described as absent | Yes | User-confirmed design change first, with information-flow and lifecycle acceptance; no implied commitment from a feature comparison |

## Corrections to earlier claims

The current comparison removes the false Pi interactive-only and Codex single-agent claims. It also corrects
TeamAgents CLI overrides, nearest trusted project configuration, SSH, explicit durable memory, release
freshness and completed briefing measurements. The older-generation release caveat was closed by D-343;
this comparison retains only the later-version freshness/rehearsal gap. The briefing experiment is a recorded null result, not still running. Statements that
formal methods or durable sessions are absent from every comparator are unsupported and are not repeated.
Historical receipts stay historical; no old benchmark score or evaluation fixture is rewritten.
