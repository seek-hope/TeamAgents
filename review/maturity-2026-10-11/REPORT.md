# Coding workflow maturity round (D-451, 2026-10-11)

Status: implementation and local verification complete; independent acceptance is a separate durable-goal
verdict. This versioned report supplies evidence, not the auditor's decision. Baseline: `ea43869d8b7058ccea63a1d96a21f60ab04e9013` (accepted D-450).
This round implements attachments/path completion, pending-input revision/cancellation, local export/search,
terminable display processes and reproducible local coding/installation workflows. It does not settle the
long-term maturity objective, establish Pi parity or measure an increase in model coding success.

## Reproduction and provenance

Run `python3 review/maturity-2026-10-11/verify.py` for offline integrity and evidence checks. The adjacent
manifest pins current source/document/probe inputs, toolchain, dependency lockfiles, every immutable receipt
and its expected status. Each receipt contains argv, cwd, UTC start, elapsed time, actual exit code, Git HEAD,
input SHA-256 and stdout/stderr hashes. `run.py --name UNIQUE -- COMMAND...` refuses reuse of a receipt name.
Streams are losslessly stored as .stdout.gz/.stderr.gz; gzip -dc restores the original bytes named and hashed
by each receipt. The verifier accepts this storage form and pins the compressed container too. This preserves
failed language fixtures and trailing whitespace without rewriting evidence or adding language exceptions.
Older dirty intermediate source hashes establish identity/differences; their complete historical source bytes
were not separately archived. Current source and committed formal material are reproducible from Git.

Environment: Linux x86_64, kernel 7.2.9, rustc/cargo 1.95.0, TeamAgents 0.2.0; exact version output is in
`receipts/baseline-corrected.stdout`. Cargo is pinned by the toolchain file and three lockfiles. Added direct
TUI dependency libc 0.2 was already transitive; no application code was added in Python. Python probes and
scripted HTTP services are test-only. Source-only analysis, TestBackend frames, actual subprocesses, loopback
HTTP, real daemon/runner and real PTY are distinct evidence classes. No new real-model, graphical Kitty,
remote provider/OAuth or cross-product success evaluation was run. All new model responses are deterministic
fixtures with fake local credentials. Historical D-450/other evaluation results remain historical.

The authorized boundary includes local code/docs/tests and Git commits, and excludes push, publication,
tags, paid trials, uploads and changes to existing user installations/configuration/sessions. Test roots and
processes are owned and isolated; existing user state roots are preserved. Leak-guard gates run serially with
other live probes. No credentials enter this report or its fixtures.

## Baseline, fixes and acceptance paths

### Attachments and pending input (C2/C4)

Before D-451, composer input was text-only and accepted user envelopes had no revision/edit/cancel API.
The implementation sites are `core/src/v2/attachments.rs`, `core/src/v2/control.rs`, `core/src/v2/store.rs`,
`engine/src/v2/attachments.rs`, `engine/src/v2/daemon.rs`, `engine/src/v2/exec.rs`, `tui/src/attachments.rs`
and `tui/src/queue.rs`. Schema 4 migrates transactionally to 5 with append-only input revisions and attachment
references. The control transaction serializes submission, edit/cancel and consumption.

The pre-implementation D-451 contract is preserved: `/attach PATH` and repeatable `exec --attach PATH` are
explicit; ordinary @ text has no hidden file meaning. Workspace candidates fill the composer, a subsequent
Enter stages a path, and only message submission snapshots bytes for i-leader. Esc dismisses selection,
`/detach INDEX` removes staging, and drafts/staging/retry identity belong to each session. Spaces and Unicode
are preserved. Approval focus, disconnected Leader refusal, interruption and exit keep their previous meaning.

Engine admission allows at most eight workspace-relative regular files, 1 MiB per UTF-8 text file, 5 MiB per
PNG/JPEG/GIF/WebP and 16 MiB total, inclusively. Absolute/traversal/remote/control paths, symlink components,
directories/FIFOs and unsupported binary types are refused. Component-pinned non-following opens, size/type
and metadata checks precede digest/sync/publication. Paths are provenance, never authority to reread after
admission. Immutable artifacts survive source edits/deletion, input edits, restart and GC. Committed retry
looks up the receipt before reading paths again. Provider expansion verifies digest/size, quotes text and
emits native image parts on Chat Completions, Responses and Anthropic; unsupported capabilities/content are
explicit errors. Loopback wire tests establish serialization, not real-provider acceptance.

`inputs list/edit/cancel` and Ctrl+Q or `/queue` expose accepted current-epoch user inputs. Mutations compare
session, recipient, envelope and expected revision atomically; late/stale/wrong-actor mutations fail. Original
text and every revision remain recorded; edits retain attachments and envelope order. Cancelled headless
input reports failure before consumption. The separate queue editor never consumes the conversation draft;
Delete captures an identity/revision and y confirms cancellation. These actions do not interrupt models/tools.

Reproduce with core input_queue, engine attachment tests, providers_fake, v2_daemon and TUI attachment/queue
suites, plus the maturity workflows/terminal probes. They cover inclusive budgets and refusals, exact retry,
original/revised history, consumption races, paging, source deletion and reopened snapshots. The real socket
restart test exposed two production defects: retained scheduler/client tasks held the coordinator lock after
shutdown; later, snapshot expansion read the member root instead of the session root. Owned task joining and
session-root reads fixed both, with failed and repaired receipts retained.

### Export and browser search (C3)

`engine/src/v2/session_export.rs` streams complete stored members, context (including covered/older epochs),
user envelopes/revisions and attachment references from one read-only SQLite transaction. Event/context
watermarks identify the snapshot; pages contain at most 256 rows. JSON and literal escaped Markdown preserve
member/epoch/tool associations and raw/unavailable references. Export never dereferences arbitrary paths,
fetches remote bytes, uploads or embeds media. Pruned/deleted rows cannot be reconstructed. The output budget
is 256 MiB with a 16 MiB record limit; overflow is an error, not truncation.

`teamagents sessions export --id ID --output FILE --format json|markdown` requires an explicit new destination.
Private create-new temporary output is synced and published without replacement. Existing paths/symlinks
are refused; cancellation/errors remove unpublished temporary output. If publication wins a cancellation
race, the actual success is returned. A later parent-directory sync failure can leave a completed destination
while returning an error; this is not partial publication. CLI SIGINT registration precedes worker start.
The actual-child signal test waits for a temporary file, interrupts, checks cleanup/nonpublication and then
exports every one of 80 large entries successfully. Other tests cover 600 rows, concurrent WAL append,
empty/archive cases, hostile literal text, broken references and registry byte preservation.

`tui/src/sessions.rs` and `tui/src/session_client.rs` add optional-browser search through the engine CLI.
Query+generation binds results; cancellation drops the owned search, advances generation and rejects stale
results. Up/Down/Home/End/PageUp/PageDown select snippets with session/member/epoch/cursor attribution; Enter
explicitly opens the session. Search never silently sends a model message or switches sessions. This is a
snippet preview, not a jump to full matching-message context or a message-branch tree. Real PTY evidence
covers Unicode, no match/hit, draft return and 35x12 resize; TestBackend tests cover additional tiny geometry.

### Terminable presentation (C5)

Before this round syntax grammars ran synchronously; a stuck parser could hold the renderer/worker forever.
`graphics.rs`, `highlight.rs`, `display_sandbox.rs` and shared `subprocess.rs` now move grammar/diagram/image
parsing into a Rust helper. Immediate plain/diff/source fallback remains visible. Media gets 750 ms and cold
code grammars 2.5 s; input/stdout/stderr each have a 16 MiB cap. Linux x86_64/AArch64 isolation sets 768 MiB
address space, 2 CPU seconds, 8 MiB stack, 32 FDs and zero core/file-write allowance. Environment is cleared,
cwd is /, inherited FDs close, fixed public fonts load, then no_new_privs/seccomp refuses subsequent file
opens, sockets and process creation before input parsing. Failure to establish isolation fails to source.
Unsupported OS/ABI is not claimed supported.

Parent polling interleaves nonblocking writes/reads/cancel/deadline/exit without reader threads. Owned direct
children are killed/waited on error or owner drop. Resize/toggle/session replacement advances generation,
stops active work, skips stale queued work and rejects stale results. Returned spans/styles/controls/width,
PNG header/dimensions/cell geometry and output counts are checked before applying; the parent does not decode
arbitrary returned image bytes again. Helper IDs are newly owned Kitty IDs.

The retained contracts are 64 KiB and 500 diagram lines inclusive, 32,768 fallback characters, cache 16,
queue 8, viewport admission, geometry after eviction, clipping and owned image cleanup, author-workspace
confinement and no remote/SVG resource fetch. Code admission is 64 KiB/500 lines/4096 bytes per line.
Actual fault helpers cover hang, crash, stdout/stderr overflow, invalid JSON, file/socket/process/memory refusal,
active cancel, queued stale generations, owner drop/reap, hostile DOT/SVG/code and next-preview recovery.
TestBackend typing/frame work continues during an actual hung helper; timeout retains source and draft.
The current twenty-media fixture resolves 20/20 items and 24/24 individual visits, including evictions.
Its longest wait was 152.52 ms; key/frame p50/p95/max were 1.07/1.93/4.05 ms and Rust body had five colors.
The accepted historical D-450 run had 24/24, 218.55 ms longest wait and 2.05/3.20/5.66 ms key/frame timing.
These runs use the same media fixture; they are not simultaneous paired performance or graphical-terminal
measurements. The example now explicitly waits for asynchronous grammar work when counting colors.

Important limits: parent filesystem reads/spawn and OS kill/wait are not a proof of kernel/filesystem liveness.
The generic pipe pump owns direct children; arbitrary Session CLI descendants are not process-group killed.
Display seccomp denies descendant creation. Explicit session Open/Delete retain engine operation ownership
when the UI detaches; search alone is cancelled/joined. Session CLI capture is bounded to 64 MiB per stream
and 120 seconds. Cold grammars may visibly fall back while pending; this is intentional.

### Real local coding and installation (C6/C7)

`workflows.py` uses real CLI, Unix-socket daemon, SQLite and shell runner with a deterministic loopback HTTP
provider. Its five scenarios independently inspect files, database rows and actual request bodies:

1. Read a broken calculator, change subtraction to addition, execute its tests, pass required check, SUCCEEDED.
2. First finish fails the required check; repair runs; second finish passes (one repair, two check rounds).
3. Three failed finishes exhaust two repairs and leave BLOCKED, with no false success.
4. SIGKILL the exact owned daemon while a real shell job is running, restart and recover its journal/result;
   the effect file has one line and journal starts is one.
5. Queue text+PNG, edit text, cancel another input, reject stale revision, delete source files, then deliver
   the immutable text/native image once with matching original/revised history and headless cancellation.

`terminal.py` drives an actual PTY through path select/cancel/fill/stage/send, queue edit/cancel confirmation,
separate draft retention, source deletion, search/no-match/hit/snippet, narrow resize and exit that leaves the
daemon alive. It is not a graphical Kitty/terminal-matrix or model-quality measurement.

`install.py` builds the current-source musl archive through the maintained release rehearsal, verifies archive,
installer and checksum bytes, installs under fresh HOME/XDG/bin/workspace, tests version/init/doctor/config
preservation, then reruns all workflows, the PTY feature probe, reconnect and session management using the
installed binaries. `installed-artifact-first` passed in 47.707 s on Linux x86_64. Its archive SHA-256 is
61d192d5fea76bfdf7af819d637b08e7d223dadbdec7904e76a0c41d50c9a3f5; installed engine is
074db9e861843d3994f6e04da9bde19139e827b0508134bc3038ebbe86bdaba5 and TUI is
f928b35169c7d11d23e528bd986556fd3bf2bcf89e27707a12455cd63e8ec29a. Archive/installer/checksums are retained
at /var/tmp/teamagents-maturity-artifacts-si9j2rtf. This initial successful rehearsal is retained separately.
After the signal/glob repairs, installed-artifact-final passed in 60.743 s. Its final archive SHA-256 is
ef122d69b21476778078d9790384e8c56b67dfc86cb2fd531b4d012db551c70b and engine is
7894ff8be3644ad78616c1321ea787533db4113b8df3b0adf9570aa8c8918306; TUI remains the hash above.
Final archive/installer/checksums are retained at /var/tmp/teamagents-maturity-artifacts-4mih4hj4. The final
installed binaries pass the same workflow/PTY/reconnect/session probes. Neither run publishes or proves
any-platform compatibility.
The v0.2.0 tag already uses v2 architecture but predates these source features; stale pre-v2 prose was corrected.

## Verification ledger (C1/C8/C9)

Every receipt, including initial failures, is enumerated and hashed by the manifest. Names are labels; argv
and logs determine scope. In particular export-engine-cli-tests ran library tests despite its name, and
export-cli-help did not establish binary freshness. Later actual CLI integration/SIGINT and installed tests
do establish those paths. Earlier direct checks remain direct checks; they were not retroactively turned
into receipts. A failed make check is never called a full green check because its Rust substeps passed.

| Gate | Evidence | Result |
|---|---|---|
| New control/attachment/provider/TUI logic | queue-core-all, attachments-boundaries-first, attachments-http-first, queue-attachments-session-root, attachments-tui-tests-fixed | passed at recorded source |
| Export and search | export-engine-first, browser-search-tui-fixed, export-real-signal-first | passed; includes actual SIGINT |
| Display faults, pipe ownership, typing | bounded-pipes-first, display-faults-first, display-input-visible-fixed, display-all-cold-grammar-fixed | passed after retained fixes |
| Coding product paths | coding-workflows-no-bytecode, terminal-features-search-key-fixed, installed-artifact-first | all five runtime cases and real PTY passed |
| New formal material | e9a0aa84; maturity-models-catalogue-formatted | two positives and twelve effective named controls |
| Full formal regression | formal-all-final / formal-controls-final / formal-kani-final | 44 positive configs / 137 intended refutations / 7 Kani proofs |
| Final ordinary gate | check-glob-fixed: make check | passed, 103.946 s; core 134 / engine 384 / TUI 73 |
| Final sandbox conditions | nobwrap-glob-fixed / broken-sandbox-glob-fixed | passed, 104.124 / 110.940 s |
| Final offline/PTY gates | offline-glob-fixed / pty-signal-fixed | passed, 162.902 / 43.430 s; 19 offline probes, no leftover daemon/scratch |
| Final workflows/installed archive | workflows-signal-fixed / installed-artifact-final | passed, 6.685 / 60.743 s; five cases, PTY, reconnect and sessions |
| Independent audit | separate durable goal submission | authoritative verdict remains outside this evidence snapshot |

D-202: formal models/configs/driving catalogue were committed as e9a0aa84 before verification/REPORT.md was
pinned. The input model has 196 generated/64 distinct states; the worker model has 9079/1601. Catalogue:
182 configurations, 36 modules, five model targets. They establish bounded abstract admission/revision/
snapshot/delivery and generation/queue/output/observed-stop/reap safety. Stop/reap is an abstract atomic OS
step. They do not prove Rust refinement, arbitrary parser termination, scheduling, filesystem liveness,
real-model success or absence of all vulnerabilities.

The maintained docs cover Q1-Q19, user controls/budgets, source freshness and operational limits. Protocol
and events references are regenerated; config/tools unchanged surfaces are audited. Direct engine/core/TUI
implementation remains Rust, Leader target/default authority and approval focus stay intact, interruption
still targets model requests, exit detaches and deletion remains identity-bound. The current discovered
suite counts are core 134 / engine 384 / TUI 73; final gate status is separate from these counts.

### Retained failures and repairs

| Receipt(s) | Observed failure | Disposition |
|---|---|---|
| baseline | unsupported exec --help invocation | corrected top-level help/version/list baseline |
| queue-tui-first; queue-tui-unit | command completion changed; accidental test insertion overwrite | selection assertion/source insertion repaired; fixed receipts pass |
| queue-daemon-cli-first | coordinator lock remained held after shutdown | scheduler/client tasks abort and join |
| attachments-engine-compile-first; attachments-tui-tests-first | temporary borrow; missing test argument | compilation repaired, boundary/UI suites pass |
| queue-attachments-daemon-first | fixture workspace missing | fixture setup repaired |
| queue-attachments-daemon-fixed | session snapshot read under member root | production root corrected; restart/native delivery passes |
| display-all-first | cold TypeScript grammar exceeded 750 ms | finite 2.5 s code deadline; media retains 750 ms |
| display-input-responsive | probe inspected an unresolved viewport placeholder | resolve first, then assert live input/source fallback |
| inputqueue-model-first; maturity-models-first | wrong simulation config; worker TLA precedence error | proper exhaustive runner and explicit parentheses; controls all effective |
| maturity-check-first | Rust checks passed, documentation counts stale | counts synchronized; final complete ordinary/sandbox gates passed |
| coding-workflows-first | same-size rapid repair reused Python bytecode | fixture tests use python3 -B |
| terminal-features-first | bracketed-pasted slash did not activate browser search key | send / key before pasted query |
| hygiene-first | literal non-English test characters violated code scan | Unicode escapes retain coverage; language scan rerun; exact log bytes retained compressed |
| check-final | socket visible before SIGINT handler registration; signal killed daemon | register SIGINT/SIGTERM before serve; daemon-signal-fixed and check-signal-fixed pass |
| nobwrap-final | older override test expected doctor success despite missing isolation backend | require valid config and exactly one isolation failure when unavailable; refusal coverage retained |
| broken-sandbox-final | an installed but unusable bubblewrap silently selected native glob after D-369 | restore D-114 refusal; original negative assertion retained; broken-sandbox-glob-fixed passes |

## Continuing work and next-round acceptance (C10)

| Priority | Area/status | Impact and dependency | Next acceptance |
|---|---|---|---|
| P1 | Real coding/collaboration outcomes: unknown for this round | determinism proves runtime only; needs separately approved model/budget | paired held-out trials below, honest failures and cost intervals |
| P1 | Real Docker/provider/image/OAuth paths: partial | fake HTTP and native serialization do not establish service acceptance; needs chosen environments/credentials | actual isolated service/container round trips, denial/revocation/restart, receipts and cleanup |
| P1 | Adversarial security: partial | display confinement/budgets tested, broader parser/tool/permission attack surface remains | independent boundary review, bounded corpus/fuzz failures and regression reproducers |
| P1 | Terminal/OS support: partial | real PTY here; no new graphical Kitty, zsh/tmux/Zellij/accessibility/remote matrix | predefined matrix with screenshots/key ownership/resize/cleanup; fail-closed unsupported isolation |
| P1 | Source release freshness: locally verified, unpublished | users of v0.2.0 lack new features; publication needs separate authorization | exact tagged artifact, checksum/install/update/rollback on clean host |
| P2 | Context/cost observability: partial | runtime usage/deadlines exist; concise user-facing attribution and compaction diagnostics remain | bounded real native-window traces, per-member/request accounting, no secret exposure |
| P2 | External editor: absent | long input ergonomics; depends on explicit draft/attachment ownership | edit/cancel/crash/Unicode/reconnect without implicit send or lost draft |
| P2 | Message branches/HTML export: absent | session fork and Markdown/JSON export exist; message lineage/HTML do not | confirmed lineage design, deterministic fork inputs and inert offline HTML export |
| P2 | MCP resources: absent | tools/transports exist; resource discovery/read/subscribe not established | resource URI/auth/size/transport/reconnect tests, explicit context injection |
| P3 | SDK/extension surface: absent | embedded/API ecosystem requires a separate compatibility/security design | versioned contracts, example embedding, cancellation/ownership and adversarial extension tests |
| Excluded | Distributed/multitenant execution, automatic cross-session injection, marketplace | outside confirmed product scope | separate complete design proposal; no implementation implied |

### Unexecuted real-model evaluation proposal

No call or spend is authorized by this proposal. Freeze a future candidate commit and the accepted D-450
baseline; if a cross-product arm is desired, separately approve and pin its exact version and matching tools.
Use 12 held-out, self-contained coding repositories/tasks: four read/repair/test defects, two multi-file
features, two required-check repair/exhaustion tasks, two queued follow-up/attachment tasks and two crash/
recovery tasks. Freeze fixtures, hidden tests, expected safety refusals and SHA-256 before any model access.
Keep six disjoint calibration tasks for harness-only debugging; never tune on held-out outcomes.

Primary outcome: complete task with hidden tests passing, no unauthorized modification and honest runtime
settlement within the cap. Secondary: wall time to all checks green, tokens and billed/estimated cost,
repair count, duplicate side effects, lost input, unsupported claims and termination reason. Refusal/exhaustion
cases pass only when they refuse/block correctly. Runtime faults are injected at predefined persisted phases,
not chosen after observing results. Independent reviewers score anonymized diffs/artifacts against a frozen
rubric; prose alone earns no success. Unknown/lost receipts count as failure in the primary analysis.

Use the same model/provider, native context window, effort, per-trial deadline and price snapshot in both arms.
Record context value and source; DeepSeek Flash uses the user-confirmed 1M configuration. Never shrink the
window. Start with two calibration pairs, then 12 tasks x 2 arms x 3 repeats = 72 held-out trials, randomized
paired order with independent fresh sessions. Each trial has a proposed 15-minute wall cap and token/spend
ceiling determined from the approved model's current prices. Suggested authorization ceiling: USD 10 pilot,
then at most USD 100 held-out total, with explicit stop at the lower of approved spend, token ceiling or wall
cap. These are proposed caps, not an estimate or permission; obtain the exact price table and pilot estimate
before requesting the full budget. Compaction/subagent usage counts, and persistent side effects never replay
on a guessed outcome. Report paired success differences with uncertainty, timing/cost distributions and all
failures; do not infer superiority from one green task or selected examples. Collaboration gain requires a
separately pre-registered task decomposition/metric and cannot be borrowed from these runtime probes.

The next round starts from these remaining gaps and its own approved scope. Acceptance here, if independently
confirmed, closes only D-451's bounded delivery.
