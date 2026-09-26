# Implementation decisions (current)

This file records the direction, boundaries and deviations that are **currently binding**: any implementation
that departs from the confirmed design is discussed with the user first. The decision log of earlier
implementations stays reachable through Git history (`git log -- docs/archive`).

## Earlier rules that still apply

| Topic | Current rule | Origin |
|---|---|---|
| Skills | user-level registration root `~/.agents/skills`; read on demand with `skill search/read`, and skill instructions never widen execution permissions | D-23, D-34 |
| Usage visibility | turn and goal usage/budget are visible in the TUI and in events | D-24 |
| MCP | both transports, stdio and streamable HTTP; calls go through the shared permission, approval, budget, cancellation and receipt entry points | D-25 |
| Context compaction | triggered by real window usage; the summary keeps the original request and acceptance criteria, and the full text stays retrievable | D-28 |
| Task priority | complex coding and long-horizon stability come first | D-35 |
| Native context | real-model evaluation always uses the model's native window and records the value and its source | D-36 |
| Install and first config | download-and-run install; `init` writes a minimal config and never overwrites an existing file | D-37; `docs/INSTALL.md` records the differences of earlier releases (≤ v0.1.2) |
| Custom providers | any compatible service is configured through `[models.*]` in `config.toml` (`protocol`/`base_url`/`model`/`api_key_env`) | D-40 (the earlier TUI's `/model` wizard went away with the old interface) |
| full_auto | user-only host shell (D-41); the default `approved_scope` runs under bubblewrap | D-41 |

## D-140 A probe reported a replay, and the assertion was measuring the wrong thing (2026-09-26)

Running the eight model-requiring probes as a batch (the offline set of D-138 covers the credential-free ones)
found one failure: `unknown_outcome.py`, A09's live half, reported `runs.log holds 2 line(s)` where D-119
recorded one — "the command ran 2 times (exactly once expected: never replayed)". Everything else in that run
was right: the operation reached `OUTCOME_UNKNOWN` with receipt class `outcome_unknown`, the task parked
`BLOCKED`, exactly one `task_blocked` notification.

Reproduction failed, which is itself the first piece of evidence: **11 runs in isolation and 10 under load**
(eight `yes` workers, then sixty on a 20-core machine, load average 966) all reported one line. The code says
the same as the reproduction: a runner that recovers a non-terminal journal turns it into `OUTCOME_UNKNOWN` and
starts nothing (`go` fires only from `READY`), and a duplicate GO is a no-op (A10) — so an operation cannot be
replayed through recovery, and two runs of the command need two **calls** of it.

That points at the assertion. It demanded *exactly one line*, which conflates "the runtime replayed an effect"
(what A09 forbids) with "the model issued the command twice" (which the design allows and nothing forbids). Two
measurements settled it: the `operations` table holds **every** tool call — one run showed seven, the leader's
`spawn` and `delegate` among them — so counting operations would be far too loose; and each operation's
`intent_json` names the tool and the command, so the calls of *this* command can be counted exactly (one, in
every run inspected).

**Fixed**: the invariant is per call of this command — `runs <= calls` (never replay) and `runs >= 1` (the
effect really happened) — printed as `calls of this command: N, runs: M` and named in the failure message.
Two controls, each reverted byte-identically: pretending the command was never called fails with "the command
ran 1 times for 0 call(s) of it: an operation was replayed (A09)", and an empty `runs` fails with "the command
never ran, so there was no in-flight effect to recover".

**Instrumentation for the next occurrence**: the command's own line now carries the shell's pid and its runner
parent (`run pid=… ppid=…`), and the probe prints every operation with its status. A future failure therefore
says whether the two runs came from one runner (a replay) or two (two calls) — the question the one observed
failure could not answer, because the probe's scratch had already been removed at exit (D-138's policy; passing
`--state-dir` is how to keep the artifact of a run).

Ceiling: the observed failure stays unexplained — one run in a batch, not reproduced in 21 attempts — and what
this entry settles is that the *assertion* was wrong, not that the product is right. A replay would now be
reported precisely, with the artifact that identifies it.

## D-139 A test measured the wrong window, and only a loaded machine said so (2026-09-26)

The D-138 gate failed once, in `jobs::tests::a_momentarily_held_lock_is_absorbed`, with "it must actually wait,
not steal the lock" — the assertion that `state_lock_waiting` waited at least 40 ms while another thread held
the coordinator lock for 60 ms. Run alone the test passed **20 times out of 20**, so the failure was
load-dependent, and the implementation is not at fault: `state_lock_waiting` loops on `File::try_lock()` with a
10 ms step until its deadline and never steals, which is exactly what that assertion exists to catch (it caught
a real steal once).

The mechanism is in the test's own clock. It took `started = Instant::now()` **after** spawning the thread that
releases the holder 60 ms later, so under load the main thread can be starved for longer than those 60 ms
between the spawn and the measurement: the clock then starts after the window has closed, the wait it measures
is ~0, and the test reports a steal that never happened. Reproduced deterministically by sleeping 70 ms in
exactly that window — the same panic, first try.

**Fixed**: the clock starts before the releaser exists. The guard keeps its meaning, verified both ways: with
the 70 ms starvation injected the test now passes (the race is gone), and a control in which the holder
releases immediately still fails the assertion (a waiter that did not wait is still caught, because a steal
returns within `LOCK_STEP`).

Evidence: 20 runs alone green before the change; the starvation control panics before the fix and passes
after; the release-at-once control panics after the fix; five consecutive runs green. The failing runs also
left three `ta-state-lock-absorb-*` directories behind, which is how the flake was spotted in `/tmp` as well as
in the log.

Ceiling: this is D-115's class — a test measuring wall-clock behaviour around a scheduling window — so only a
load run exposes it, and `make test`'s scratch guard cannot report it (the suite aborts at the first failing
test, so the guard's counters never run). The module's `temp_path` directories are still removed at the end of
each test rather than on drop, so a panicking test leaves one; that is D-131's per-module cleanup ceiling, not
this item's.

## D-138 What the probes left behind, and what they mis-reported (2026-09-26)

The probes in `review/dogfood/` are the repository's most direct evidence — they drive the built CLI, real
session daemons and the real TUI — and they are run by hand, repeatedly. Running the credential-free subset as
a batch exposed four things about them rather than about the product, and one about the product's *reporting*.
There is now a runner for that batch, `python3 review/dogfood/offline.py` (`make probe-offline`).

**Every probe left its scratch behind.** All 31 built a scratch root under `TMPDIR` and none removed it: six
runs of the offline set added six directories (`ta-boundary`, `ta-budget`, `ta-truncation`, `ta-latency-*`,
`ta-panels`, `ta-reconnect`). It is the defect D-131 fixed for the test suite and its `make pty` follow-up, and
it has the same consequence: a directory per run per probe until `TMPDIR` fills. Each probe now removes the
**default** scratch at exit and leaves an explicit `--state-dir` alone — which is also the escape hatch when a
probe fails, and it is why the same change did not hide the `deadline.py` finding below. Verified: the offline
set now reports `new scratch none`, and a re-run also cleaned five stale directories from before the change.

**Six probes never stopped their daemon** (`deadline`, `instructions`, `mcp`, `mcp_http`, `truncation`,
`two_gates`); their 25 siblings had registered `stop_daemon` with `atexit` and they had not. Measured:
`truncation.py` left **two** live daemons (`--state-root /tmp/ta-truncation/{invisible,visible}/root`), which
survived the run and only exited later by themselves. They now register the same helper; `atexit` runs it
before the scratch removal (LIFO). Verified: 0 daemons before and after, and the offline runner fails the run
if the count grows.

**Three probes documented as needing no credential did need one** (`boundary`, `tui_panels`,
`tui_reconnect`): the daemon refuses to boot without a value for the config's `api_key_env`, even though no
model is called. And `tui_panels.py` went further than its docstring claimed — it delegated a task to a live
member, which **began a real model request** (measured with a live credential: `request_began`, then the `c`
key abandoned it, `CANCELLED`). The three now supply a value when the environment has none, and
`tui_panels.py` holds the member across the delegation, so the session's `model_requests` table stays **empty**
— the turn its subject never wanted. Its `t`/`n` assertions now check that those keys change *nothing*, which
is both stronger than "is ACTIVE" and independent of the member's state. Verified by running the whole set with
`DEEPSEEK_API_KEY` and `KIMI_API_KEY` unset: 7/7 ok.

**`tui_panels.py` crashed instead of reporting** when the daemon failed to start: it read `daemon.log` through
the handle it had opened for writing (`io.UnsupportedOperation: not readable`). It reads by path now. The
credential-free control is what surfaced it — a probe that only ever ran with a working credential never took
that branch.

**`deadline.py` — A35's live evidence — mis-reported a model choice as product failure.** A run whose first
turn *settled* the goal (the model called `finish` instead of only replying, the intermittency D-129 records
for the A27 probe) printed seven FAILs: the deadline was not refused, a model was reached, the `--check` ran.
All of that is *correct* behaviour for a closed goal — the deadline gate refuses a request for a goal that is
still running — so the scenario had simply never started. The probe now asserts that premise, sets it up again
once, and otherwise prints `SETUP:` with the reason and exits 1. Measured: the re-run passes on attempt 1
(`exit=0`, refusal in 0.3 s, one model request, `goal_deadline_refused`, leader PARKED with the reason), and a
control that makes the premise unsatisfiable exercises the retry and prints SETUP.

Ceiling: the runner covers the credential-free subset only — the model probes still run one at a time — and it
looks for *new* `ta-*` names and daemons, so a probe that leaks under another name is invisible; the scratch
policy is "default removed, explicit kept" and a probe can ignore it.

## D-137 The confirmed requirements had no evidence trail (2026-09-26)

The baseline's §1 lists the requirements the user confirmed (Q1–Q19) and D-42 closes with "the full requirement
mapping and acceptance are in the design baseline". There was no mapping. `docs/ACCEPTANCE.md` carried
per-**scenario** evidence for A01–A36, and **fourteen of the nineteen requirements appeared in no document but
`docs/DESIGN.md` itself**; §12's scenarios and §1's requirements were never joined. The requirement an
experiment actually settled — Q16's "collaboration must show a reproducible gain" — was not connected to the
experiment that measured it, and `ACCEPTANCE.md` did not refer to the evaluation at all.

That is the gap the A-matrix's discipline exists to prevent one level down: every scenario has a row with
evidence, while the requirements those scenarios serve had none, so "is the baseline met?" could only be
answered scenario by scenario, and a requirement could go unexamined without anything noticing.

**Added**: `docs/ACCEPTANCE.md` now has a "Requirements (Q1–Q19) and their evidence" section — one row per
requirement, naming the acceptance items, decisions, probes or measurements that cover it, in the A-matrix's own
marker vocabulary (✅ automated evidence, 🔶 partial, ⚠ not implemented). `docs/DESIGN.md` §1 points at it,
closing the loop D-42 promised.

Writing it recorded a real outcome rather than only cross-references. **Q16's collaboration half is 🔶 measured
and not confirmed**: the pre-registered experiment in `review/eval/r2-p6/REPORT.md` passed H1 (single-instance
behaviour does not regress — 135/135 accepted, per-task paired difference 0) and did not confirm H2 (a
reproducible collaboration gain — in all 99 group-C trials the model stayed a team of one, which the design
explicitly allows). That is now a known-gap bullet in `docs/ACCEPTANCE.md`, written as a gap in the *evidence*
rather than a violation of the runtime: what would close it is a task set or an instruction shape that makes
delegation the shortest path, which is an experiment to design.

**Guarded**: `review/requirement_trace.py`, in `make hygiene`. Every `Q<n>` in the baseline's §1 table must have
a row; every row must name a `Q<n>` the baseline lists (a row for a requirement that no longer exists is the
same defect in reverse); every row must cite an acceptance item (`A01`–`A36`), a decision (`D-<n>`) or a path
that exists, so a row cannot be a placeholder; and a missing section is reported as a finding — the first draft
died with a traceback when the heading was renamed, which its own control caught.

Evidence: the script reports "19 confirmed requirements in the baseline; 19 evidence rows" and exits 0. Five
controls, each reverted byte-identically afterwards: dropping Q9's row fails ("Q9 … has no evidence row"),
adding a `| Q20 |` row fails ("the baseline does not list it"), citing a non-existent path fails (Q12), emptying
Q7's evidence cell fails ("names no acceptance item, decision or path"), and renaming the section fails
(missing section plus nineteen missing rows).

Ceiling: coverage and shape, not adequacy — a row citing an unrelated acceptance item passes, so the judgement
stays with whoever writes the row (`--list` prints every row for review); and the trace is one-way, from
requirement to evidence, so an acceptance item that serves no requirement is not reported.

## D-136 The parser kept a removed subcommand's flags, and the guard looks at the parser now (2026-09-26)

Checking the product documents against the CLI's help text (D-135) turned up the same question one layer down,
in the parser: three match arms were gated on `args.command == Some("sessions")` — `--history-days`, `--days`
and `--dry-run` — and the two fields they wrote (`history_days`, `dry_run`) are read nowhere in the crate. No
invocation could use them, because `sessions` itself is refused ("this entry point is no longer supported"):
they were the flag surface of the removed `sessions prune`, left behind when the entry point went (D-73/D-78,
whose rule is that a removed surface leaves nothing behind). `--days` was even writing into `args.timeout`,
a copy-paste artefact of the same removal.

Two smaller dead shapes fell out of the same reading. The dispatch arm `Some("validate") | Some("sessions") |
Some("serve") | Some("repl")` could never see `Some("repl")`: that word is not accepted as a command, so
`teamagents repl` was refused as a bare word (measured — it prints "is not an entry point"), which made one of
the four alternatives unreachable. And `-v/--verbose`'s *refusal* arm looked unadvertised to a naive scan
because its message is a multi-line string literal: the same shape that made D-130's audit count a doc
comment as a call, and the reason the check below compares arm *patterns* exactly rather than any line that
starts with a quote.

**Fixed**: the removed entry points (`serve`, `validate`, `sessions`) are refused where their word is read,
with the message they already had, so the command is named rather than whatever flag followed it; the three
flag arms, the two fields and the dead dispatch arm are gone. Measured after the change: `sessions`,
`sessions --dry-run` and `sessions --history-days 30` all still print "teamagents sessions: this entry point is
no longer supported…" and exit 2, and `repl` still gets the bare-word refusal — the refusal *site* moved, the
user-visible behaviour did not. `cli::a_bare_word_and_verbose_are_refused_without_starting_a_session` now pins
all of it; it had no coverage of the removed entry points at all before.

**Guarded**: `review/doc_flags.py` gained the parser half of its check — *a flag the parser accepts must be
advertised in the help text or named in a refusal message*. Accepted-and-then-ignored is exactly the defect
D-75's rule forbids for config keys, applied to flags. It needs no allowlist: `--plain`, `--resume`, `--team`
and `--verbose` are parsed on purpose so their refusal can name them, and each is named in one. Before the fix
the check reported precisely the three silent flags; after it, "the parser accepts 20 flags; 16 of them are
advertised, 0 are accepted silently".

Ceiling: the parser half reads match-arm *patterns*, so a flag accepted through some other shape (a loop over
a literal array, say) is invisible to it; it reads the source, so it cannot see whether a flag is honoured at
runtime; and it does not model per-flag *reachability* (a flag gated on a subcommand that exists but never
sets the field it guards).

## D-135 The user guide showed a flag this build does not serve (2026-09-26)

D-73's rule is that an unserved argument is a *refusal naming the word* — so a flag a document shows either
exists, or the reader meets an error the first time they copy the line. Nothing checked the two documents a
user reads first. A scan of every backticked `--flag` in the tree found one: `docs/USER-GUIDE.md` §6 described
the earlier-release cleanup as "list first, delete only with an explicit `--apply`", and `--apply` is a flag of
no entry point. The cleanup was a one-off migration performed against a written inventory, which is how
`docs/ACCEPTANCE.md`'s upgrade notes already described it; §6 now says that in the same words, and adds what
matters to a reader — nothing in this build deletes their data on its own.

The scan also showed why the other documents are *not* in scope, and each for a reason worth recording:
`docs/ACCEPTANCE.md` and `docs/INSTALL.md` record the flags this build **removed** (`--plain`, `--resume`,
`--team`, `--verbose` — naming them is the point), `docs/PRODUCT-COMPARISON.md` names other products' flags
and one proposal (`--last`, `--stream-json`), and `docs/DECISIONS.md` and `docs/DEVELOPMENT.md` quote this
repository's own tools (`--list-known`, `--stub-bwrap`, `--locked`). The flags in the two product documents
that belong to the *toolchain* rather than to this build are five, and the script lists them with their
reason (`--offline`, `--locked`, `--manifest-path`, `--release`, `--bin`).

**Guarded**: `review/doc_flags.py`, in `make hygiene`. Its authority is the CLI's own help text — the `HELP`
constant in `engine/src/main.rs`, which is what `teamagents --help` prints — read from the source so the check
runs in `make hygiene` on a tree that has not been built. The served set it derives is exactly the sixteen
flags the binary prints.

Evidence: the script reports "95 flag mentions in README.md, docs/USER-GUIDE.md; the CLI's help lists 16 flags
and 5 belong to the toolchain" and exits 0. Two controls, each reverted byte-identically: restoring the
pre-fix sentence fails at `docs/USER-GUIDE.md:445: --apply is not a flag this build serves`, and adding
`--plain` to a README command fails at `README.md:136`.

Ceiling: it compares the documents with the help text, not the help text with the parser, so a flag the help
advertises but the parser refuses would pass here — that would be a different finding, in
`engine/src/main.rs`. The check is also one-directional: a flag this build serves and the documents never
mention is fine.

## D-134 The Chinese README is a mirror, and two things had stopped mirroring (2026-09-26)

`README.zh-CN.md` is the repository's single non-English document (AGENTS.md), which makes it a *translation*
of `README.md`: everything that does not need translating has to stay equal to the English file. Two things
had drifted.

The documentation table lost a row: `docs/PRODUCT-COMPARISON.md` was added to the English table by `35dc328`
("compare the product with Codex CLI, Pi and Hermes") and never to the Chinese one, so a Chinese reader's
index silently lacked the document the user's own direction produced. And the file carried **two sections
under the same heading** — the quick-start heading, used twice, the earlier a shortened copy of the later one
and present since the file was created (`b6a8948`) — which is why the Chinese README had ten `##` headings to
the English one's nine.

Nothing caught either: `citations.py` only asks whether a cited path resolves (and the lost row's path
resolves elsewhere in the tree), and `language-check` deliberately exempts this file. The one document with no
structural check was the one in another language.

**Fixed**: the duplicate section is gone (its five lines are a subset of the later Quick start — `init`,
`doctor`, `--cwd`, the TUI launch and an `exec --json` example all appear there), and the comparison row is
back in the table in the English row's position, with its description translated. The Chinese README now has
the same nine `##` headings and the same eight table rows as the English.

**Guarded**: `review/readme_zh.py`, in `make hygiene`, asserts the three invariants that are language-
independent — 1. the heading-level skeleton (sequence of `#`/`##`/`###`), so a section added, removed or
re-levelled on one side is a finding; 2. the in-repository link targets; 3. the CLI surface both files show
(`teamagents <verb>` invocations and `--flag` tokens), because neither commands nor flags are translated. The
translated text is deliberately not compared, and the one intentional asymmetry — each README links to the
other ("read this in Chinese/English") — is an explicit exception in the script.

Evidence: the script reports "12 headings, 16 in-repo links and 5 verbs / 17 flags on each side" and exits 0
on the fixed tree. Four controls, each reverted byte-identically: dropping the comparison row again fails on
the link, adding a section fails on the skeleton, renaming `--timeout` fails on the flag set, and
`teamagents exec` → `teamagents execute` fails on the verb set.

Ceiling: this compares structure and surface, not meaning. A paragraph that drifts from its English original
while keeping its section and links is not caught, and neither is a link that is equally stale in both files.

## D-133 The config reference said the project file was read; nothing reads it (2026-09-26)

`docs/CONFIG.md` opened its trust story with "A project file *is* read for `models` and `tools`, and for
`skills_paths`/`instruction_files` only with `[permissions] trust_project_tools = true`". Two more places said
the same in passing: the user guide's MCP bullet ("a cloned project's tools load only with `[permissions]
trust_project_tools = true`") and the A25 acceptance row ("`[tools.<name>] kind = "mcp"` in the user config
(or a trusted project config) loads at session start"). All three were false. The daemon, TUI and `exec` load
the **user** config (`load_user_config(&user_config_path())`); `load_user_config_for`, the re-parse that merges
`<cwd>/.teamagents/config.toml` under those trust rules, is called from nothing but its own unit tests — so a
cloned repository cannot change a session at all today. `docs/ACCEPTANCE.md`, `docs/USER-GUIDE.md` §2 and
`docs/PRODUCT-COMPARISON.md` already said exactly that, so the tree contradicted itself and a reader could
not tell which half to believe.

The drift has an honest cause: the loader *exists*, is implemented, is tested and is described in D-74's design
note, so prose about what the loader does reads like prose about what the product does. That is the same shape
as D-130's "a test drives it" — a fact that is one step away from the behaviour being claimed.

The three documents now say the truth (reusing `USER-GUIDE.md` §2's wording), and **no code changed**: wiring
the loader is a decision recorded in `docs/ACCEPTANCE.md`'s known gaps, because it changes what a cloned
repository can influence (including a malformed project file failing the session start). What the tree gained
instead is a check that the claim cannot drift again: `python3 review/project_config_claim.py`, in
`make hygiene`. It computes the fact from the code — is there a **call site** of `load_user_config_for` in
`core|engine|tui/src` outside a `#[cfg(test)] mod`? — and then requires every document to agree with it: four
negative sentences must be present while it is unwired, and the three wordings that claimed the merge is live
must be absent. Wiring the loader therefore fails the check until the sentences and the list change together,
which is the point: one fact with several statements, not several opinions.

Evidence, with the two controls the D-122 rule asks for (each reverted byte-identically afterwards):
`project_config_claim.py` prints "not called by the product's own code (0 production call site(s))" and exits
0 on the fixed tree; re-adding "A project file is read for `models` and `tools`." to `CONFIG.md` fails with
that sentence named; and inserting one production call site (`let _ =
crate::config::load_user_config_for(std::path::Path::new("."));` at `cli.rs`'s prod call to
`load_user_config`) makes all four documents report as stale and prints the note naming what to rewrite. The
comparison is word-based, so markdown emphasis and line wrapping cannot hide a sentence — both were hit while
writing it (`*not* read yet` and a sentence split across two lines).

Ceiling: the positive half is a blacklist of the three wordings that were wrong, so a *new* way of claiming the
merge is live is not caught; and the fact is a call-site count, so a call reached through dynamic dispatch
would look like no call at all.

## D-132 The Kani proofs were re-run; the toolchain was there all along (2026-09-26)

D-122 recorded a ceiling and `verification/REPORT.md` §0 repeated it: "the Kani toolchain is **not installed in
this environment**", so `make verify-kani`'s fixed assertion was unexercised here and the paging-arithmetic
proof stood only "by subject identity" (the verified commit is still the only commit that touched
`page_span`). Both statements were false. Kani 0.68.0 with CBMC 6.11.0 is installed at `~/.cargo/bin/kani` —
the exact prefix the Makefile's `KANI_PATH` already prepends — and `verification/kani/target/` had last been
written on 2026-09-25, so the proof had already run in this environment once. Nothing in the tree checked
whether the binary was there; the ceiling was a guess written into the report and copied into the decision
log, where it would have kept the layer unverified indefinitely.

`make verify-kani` now reports `Complete - 3 successfully verified harnesses, 0 failures, 3 total` (~16 s
including the build), so the arithmetic is verified on the **current** sources rather than carried over. The
harnesses compile the repository's own `core/src/kernel/types.rs` through `#[path]` and prove the published
`page_span` (`page_output` calls it) — two properties for every `usize`, one within the `#[kani::unwind(12)]`
bound.

The target had never been shown able to fail, which is D-122's own rule, so it got a negative control: with
`page_span` mutated to `limit.min(total.saturating_sub(offset)).max(1)`, the same command reports `1
successfully verified harnesses, 2 failures` and exits 1 with the target's own message; the mutation was then
reverted byte-identically (`git diff` empty) and the green re-run repeated. The proofs are therefore sensitive
to their subject, and the assertion D-122 added is exercised rather than assumed.

Evidence: the two runs above, and `verification/REPORT.md` §0 and the evidence table updated to say what
happened (`re-run 2026-09-26`, with the control that flips it to exit 1) instead of why it need not happen.

Ceiling: the toolchain lives at a user path (`~/.cargo/bin`), not in the repository, so a machine without it
still fails loudly with the Makefile's install hint rather than silently skipping the layer; and the looping
harness keeps its unwinding bound, so only the two loop-free properties claim every `usize`.

## D-131 The test suite filled `/tmp` until the machine stopped (2026-09-26)

`make check` began failing with `create schema: disk I/O error`, `make pty` with `printf: write error: Disk
quota exceeded`, and then *any* command failed before it started: the work sandbox could not register a
mount target (`failed to register synthetic bubblewrap mount target /tmp/.git: Quota exceeded (os error
122)`). The machine's `/tmp` tmpfs was at 13 GB of 16 GB, and **4,824 `ta-*` directories (6.7 GB)** were
sitting in it — 335 copies of `ta-ws-worktree`, 297 of `ta-tui-daemon-version`, and so on.

Every copy came from a test helper: `workspace::tests::temp` (`engine/src/workspace.rs`) and
`daemon_client::tests::fake_daemon` (`tui/src/daemon_client.rs`) built `<TMPDIR>/ta-<name>-<pid>`, cleared a
*stale* copy before creating it, and never removed it afterwards. Because the name carries the process id,
each run added a fresh copy rather than reusing one, so the directories accumulated one per tag per run
until the tmpfs was full. Two fixes:

- both helpers now return a `Scratch` guard that removes its directory when it drops, so a test leaves
  nothing behind even when it fails;
- `make test` counts `ta-*` entries in `TMPDIR` before and after the suite and fails if the run added any —
  the scratch analogue of the daemon-leak guard D-111 added, with the same shape of message.

**The tidy-looking fix is rejected, and the reason is measured.** Pointing the suite's `TMPDIR` at a scratch
root the recipe then removes — the obvious way to bound the whole thing — broke **26 daemon tests**: a daemon
socket path already sits near Linux's 108-byte `SUN_LEN` limit (`/tmp/teamagents-v2-daemon-exec-budget-
refused-<uuid>/state/daemon.sock` is 100 bytes), so one more path component makes `bind` fail with `path must
be shorter than SUN_LEN`. A guard that costs nothing beats an isolation that cannot fit; the recipe's comment
records this so the same idea is not "fixed" back in.

Evidence: `make check` is green and reports `ta-* before: 0 / after: 0` (the guard's own counters) on the
fixed tree; with the leak live, the same gate died at `check exit=2`. The 4,824 stale directories were removed
by hand after confirming no daemon or runner was live — they were this project's own test scratch, and the
suite's own names (`ta-ws-*`, `ta-tui-daemon-*`) say whose they were.

The same class had a second source. The PTY smoke (`tui/scripts/pty_v2_smoke.py`) built its fake-daemon
workdir with `tempfile.mkdtemp(prefix="ta-v2-pty-")` and never removed it, so `make pty` left one directory
(and the `daemon.sock` inside it) per run — measured going from 1 to 2 across one run, with **no** daemon left
behind. It now registers `shutil.rmtree` with `atexit`, and the `pty` recipe points `TMPDIR` at the
`check_dir` its own trap already removes, so the run's config, state and temp all live under one root that
goes away. Re-measured: `make pty` leaves 0 directories, 0 daemons, and still prints `pty v2 smoke: ok`.

Ceiling: the guard counts `ta-*` names under `TMPDIR` only, so a test that leaks under another name, or
outside `TMPDIR` altogether, is still invisible; and cleanup is per test module, so a new helper has to adopt
the guard itself (nothing enforces that but review).

## D-130 The surface nothing calls was three kinds of "mention" bigger than the audit said (2026-09-26)

D-86 built `review/dead_code.py` and recorded its blind spot in prose: a name a *test* mentions counts as a
use, so "`merge_branch` looks used only because a test drives it". That caveat was never mechanized, and it
was not the only one — the audit counted any mention as a call, and three kinds of mention are not calls. A
name mentioned in an integration test or an example, a name mentioned inside a `#[cfg(test)] mod`, and a name
mentioned in a **Rust doc comment** all read as "used" while the product's own code never calls them (prose
under `docs/**` was already excluded in D-86, but `///` prose was not).

The audit now splits every mention by whether it is a call: only `core|engine|tui/src` outside a
`#[cfg(test)] mod`, plus the scripts and `Makefile` that drive the CLI, count. It reports three buckets —
`uncalled`, `test_only`, `doc_only` — each with its own hand-kept allowlist that states the decision keeping
the name, and it runs in `make hygiene` (it counts tokens in one pass per line now, ~4 s instead of ~55 s).
It also scans only the product's own definitions: the old "409 public items" included ones defined in test,
example and bench files, and one definition is itself `#[cfg(test)]`-gated.

First run over this tree: **4 findings**, all gone now.

| Removed | Why it was dead |
|---|---|
| `tools::member_executor`, `tools::workspace_executor` | public convenience constructors over `member_executor_with_control` / `workspace_executor_with_control` — which is what `V2Toolkit::new`, the product's own path, calls. Their only callers were the probe tests, and the inner functions take the member's real `ArtifactPaths`; the shared-artifact shape was the retired v1 entry. Deleted, exactly as D-78 deleted `shell_run_host` and `workspace::is_dirty`; the probe tests keep a small local adapter |
| `theme::SELECT_BG`, `theme::HOVER_BG` | palette constants nothing paints, whose doc comments claimed they "mark the selected and hovered row" — D-86 deleted `ZEBRA_BG` for the same reason. Deleted, and the module doc no longer lists them |

What the new buckets found *already known* is now allowlisted instead of looking used, each with its reason
printed by `--list-known`:

- `doc_only` — `driver::cancel_turn` (D-78 kept it as D-63's parked substrate; the driver's own doc was its
  only other mention), and `Goal` / `Artifact`, the goals/artifacts row shapes of DESIGN §4.1 that the code
  reads through SQL, which is the reason the four siblings in `KNOWN_UNCALLED` already carry.
- `test_only` — `run_reference` (the eval group-A reference loop its tests drive), `inject` and
  `with_stream_stall` and `with_control` (each documented as a test hook where it is defined),
  `sandbox_usable` (the tests branch on the verdict; `doctor` reports the reason through `sandbox_state()`,
  which its doc had mis-stated and now says), `load_user_config_for` (the project-config open item in
  `docs/ACCEPTANCE.md`), `merge_branch` (D-76) and `load_image_reference` (below).

Two comments were corrected because the audit's findings are what they described. The providers' image notes
said `view_image` "stays on the legacy chat.rs path" — a module that no longer exists (D-78's
`validate_web_bindings` class). They now say what v2 does: `view_image` returns a reference in its receipt
and nothing loads it into a request, because the request-build-time loader `tools::load_image_reference` has
no caller; that loader carries the `ponytail:` note D-78 gives parked substrate, naming what would wire it.

Evidence: `python3 review/dead_code.py` reports `0 uncalled, 17 known and allowed` and exits 0, and it can
fail — before these changes the same command reported the four names above and exited 1. `make check` and
`make language-check` are green.

Ceiling: a mention is a name match, not a resolved call, so an item reached only through a macro, a trait
object or a string in a config can still look called (the detector's note that "the report is a starting
point, not a verdict" stands); and the allowlists are hand-kept, so a new bucket member is reported until a
human records why it stays.

## D-129 The A27 probe told a model's prose apart from a regression (2026-09-26)

`review/dogfood/providers.py` asserted `task_delegated` + `task_started` + `task_completed` for every run. But
the worker in that probe is a real Kimi model, and a model sometimes ends its turn with prose instead of calling
`finish`. Then the task never settles and a leader that verifies the artifact itself can still settle the goal
`SUCCEEDED` with that task open — the recorded known gap ("A model that stops settling its task leaves a visible
wait", `docs/ACCEPTANCE.md`, `complete_goal` checks open **operations**, not open **tasks**). So the probe went
red on a documented product limitation, and a red probe that reports nothing about what the build did wrong is
worse than no probe.

The rule is now explicit and *narrower*, not weaker. `classify_task_result` returns one of `completed` /
`known_gap` / `unexpected`: `task_completed` is the expected event; a missing one is accepted **only** for the
recorded shape (artifact exact **and** goal `SUCCEEDED` **and** the task still `PENDING`/`RUNNING`) and printed
as the gap, citing the clause; every other missing-event shape is still a failure. The blanket assertion was
replaced by one that also rejects a settled task with no completion event, or the gap shape behind a `BLOCKED`
goal — cases the old single `in`-test could not separate.

Evidence. The rule is checked without a model, a network or credentials: `providers.py --self-check` classifies
six shapes, and a mutated classifier (returning `completed` unconditionally) makes it fail, so the check can
itself fail (D-122's lesson). The classifier replayed over the two real sessions the earlier red runs left on
disk — `/tmp/ta-providers-d128/root` and `/tmp/ta-providers/root`, both `task write_answer_txt RUNNING`,
`goal-s-main SUCCEEDED`, artifact exact, no `task_completed` — now returns `known_gap`, which is what those runs
actually were. Two fresh real-model runs are green: DeepSeek Flash leader + `k3-256k` worker, goal `SUCCEEDED`,
artifact exact, `task_completed` present, 8 requests / 15.7 s and 8 requests / 22.6 s (2026-09-26).

Ceiling: the classifier reads only the events, the task rows, the artifact and the goal's status, so it cannot
tell a prose answer from any other cause of a missing `task_completed` beyond that shape. Whether the runtime
should *refuse* a settlement while the goal's delegated tasks are open remains a design question about team
semantics and stays with the user (`docs/ACCEPTANCE.md`).

## D-128 The config reference is generated, and the audit got stricter (2026-09-26)

The config file is the first thing a user edits and its reference was scattered: part of it in `USER-GUIDE.md` §2,
part in `INSTALL.md`, the rest discoverable only in `core/src/models.rs`. `docs/CONFIG.md` is generated from those
structs by `python3 review/config_reference.py --write` — **45 keys** in the six tables this build ships, each with
its type, what it holds when the key is absent, the files that *use* it, and the field's own doc comment as its
meaning. The introduction states the two things a reader must not misread: the absent value is the Rust default,
not always the effective one (an `Option` key is usually read with `unwrap_or(…)`, so `tool_timeout_s` is unset in
the table and 120 seconds where it is used), and the reader column is a search, not a proof.

Writing it exposed a flaw in the audit that shares this extraction (`review/config_keys.py`): "read by" was a
**bare-name** search, so `[models.<name>] provider` was reported as read by the kernel's own unrelated `provider`
fields — three of its nine "readers" were other structs' declarations. Both scripts now count a *use*
(`.key` or `["key"]`). Two keys lose every reader under the stricter form, and both are honest: `retention` is
parsed and reported as not applied (D-75's known gap), and `deadline_minutes` is applied **by the loader** —
`config.rs` turns it into each goal's absolute deadline, which the audit's plumbing exclusion cannot see. Both are
in the known list with those reasons, so `config_keys.py` still reports 0 unserved and 6 known.

`make hygiene` runs the reference's check mode: a new key, a renamed key, a retyped one, a changed doc comment or
an unmapped config struct fails until regenerated. Controls (run): adding `probe_key` fails the reference *and*
makes the audit report it as unserved (1 unserved, 6 known); renaming `instruction_files` fails the reference with
the new name as undocumented.

Ceiling: the reader column is still a name-based search (a differently named accessor shows nothing), only the
tables this build ships are listed, and a key that is consumed purely inside the loader has no reader column to
show — `doctor` is the surface for what a session resolved.

## D-127 The tool surface had no catalogue either (2026-09-26)

D-125 catalogued the events and D-126 the protocol; the third surface in the same state was the one a user cares
about most — what the model may actually *call*. This build assembles it from three schema functions
(`kernel::builtin_tool_schemas()`, `kernel::collaboration_tool_schemas(actions)`,
`reference::basic_tool_schemas(web, skills)`) and documented none of them: a user writing instructions saw the
tools only after a run, and the JSON schemas in the code were the only description of what each one takes.

`docs/TOOLS.md` is that catalogue, generated by `python3 review/tool_catalogue.py --write` from those three
functions: **17 tools** (`finish`, `read_history`; `send`, `delegate`, `spawn`, `wait`; `ls`, `read_file`,
`write_file`, `edit_file`, `delete`, `glob`, `grep`, `shell`, `web_search`, `web_fetch`, `skill`), each with the
layer that offers it, the model-facing description verbatim and a parameter table with the required ones marked.
The document also states the three layers that decide a member's subset — the profile's tools, the instance's
grants (D-61) and the session's bindings (D-74, whose MCP tools are discovered from the server as
`<service>_<tool>` and therefore cannot be listed) — and that the dispatch boundary re-checks regardless of what
was offered (§6.1).

`make hygiene` runs the check mode, so the document cannot drift: a renamed tool, an added or renamed parameter
or a reworded model-facing description fails the gate until it is regenerated. Controls (all run): renaming
`FINISH_TOOL`'s string, adding a `grep` parameter and rewording `ls`'s description each fail with the
regeneration hint.

Ceiling: only the static schemas are listed — MCP service tools are per-session and dynamic, and the dispatch
side (which name reaches which executor) stays with the tools' own tests rather than this document. Descriptions
are quoted verbatim, so an intentional wording change is a one-command regeneration rather than an edit.

## D-126 The daemon protocol had no catalogue either (2026-09-26)

D-125 documented the event log; the requests and replies that produce it were in the same state — DESIGN.md calls
the surface "one JSON-lines protocol (`/v1`)" and states its guarantees (handshake, `command_id` dedup, the event
watermark) without listing a method, while the TUI, `exec`, the CLI verbs and every probe speak it by hand.
`docs/PROTOCOL.md` is that catalogue now, generated from the daemon's two dispatchers:

* **6 read methods** (`checkpoint`, `events`, `history`, `tasks`, `approvals`, `grants`) with the parameters each
  arm reads, the reply keys it builds, and the dispatch site. The generator also cross-checks the arms against the
  read-only whitelist in `handle` — the list that decides which methods are answered from the database without the
  supervisor — and reports an arm that is missing from it.
* **40 commands** from `fn dispatch` in the control plane, with the parameters each handler reads, whether the
  handler takes an identity (32 of 40 do, i.e. re-check who may do this), and the dispatch site.

Plus the parts no extractor could invent: the framing (one JSON object per line, greeting first with `server`,
`protocol_version` 1, session, state root, permissions and workspace), the request shape (`request_id`,
`command_id` required for commands and how a retry reuses it), the reply shape (`ok` with `result`, or `error` as
a sentence the clients turn into exit codes), and the statement that a `result` is the handler's own JSON, so a
client should treat keys it does not know as opaque.

`make hygiene` runs the check mode, so the document cannot drift: a method added to either dispatcher without a
row, a row whose method no longer exists, a changed parameter list or a hand edit fails the gate. Controls (all
run): renaming `history` in the read dispatcher reports both the new name as undocumented and the old one as
having no arm; deleting the `submit_input` row reports it as missing; renaming `cancel_task` in the control
plane's dispatcher reports the same pair for commands.

Ceiling: the parameter column lists what the handler *itself* reads, so a field a helper reads is missing from the
row (a gap, never a wrong entry — the same shape the D-124 detector had to widen its scope for), and the tables
come from `match` arms, so a method constructed at runtime would be absent entirely.

## D-125 The event log had no catalogue (2026-09-26)

The event log is a product surface — the daemon's `events` request is how a client reconnects from a watermark,
`exec --json` builds its report from it, probes assert on it, and an operator can read `session.sqlite` — and this
build emits **45 kinds** of them. The design describes the *shape* (a per-session sequence, a scope, a payload)
but names no kind, and neither did any other document: a client author or a curious user had to read
`core/src/v2/control.rs`.

`docs/EVENTS.md` is that catalogue now, and it is **generated** from the code rather than written by hand:
`review/event_catalogue.py --write` reads every `event(...)` call site in `core/src` and rewrites the table with
each kind's scope, payload keys, emitting site and the files outside `core/src` that mention it. `make hygiene`
runs the check mode, so the document cannot drift from the code in either direction:

* a kind emitted but not listed,
* a listed kind with no call site any more,
* a changed payload key (the table would no longer match the generator), or
* a hand edit inside the generated block,

each fail the gate. Controls (all three run): renaming `goal_created` in the code reports
`goal_created_probe: emitted by core/src/v2/control.rs:1351 but not in docs/EVENTS.md` **and**
`goal_created: documented but no event(…) call site exists`; deleting that row from the document reports the
first of those; renaming a payload key reports the table drift.

The last column is a text search over the *consumers* — `tui/`, `engine/src`, the test suites and the probes
under `review/` — so the document also says who reads what. Prose is deliberately not a reader: this entry
names nine kinds, and counting a document that merely writes about a kind would flip the column whenever the
log is discussed (the first run of the generator did exactly that against an earlier version of this entry). **9 of the 45 kinds have no reader in this tree** (`artifact_abandoned`,
`artifacts_gc_claimed`, `goal_created`, `inbox_drained`, `instance_terminated`, `operation_cancel_requested`,
`operation_cancelled`, `operation_unauthorized`, `request_cancelled`) and are marked *observability only*. That is
an honest column value, not a defect: the log is a supported way to observe them, and the ones a probe or a test
does assert on are now named in one place.

Ceiling: the arguments are split by commas and payload keys are read from a `json!({…})` literal, so a payload
built at runtime is invisible to the generator (it would then be missing from the table rather than wrong); the
readers column is a text search, so a reader that matches a kind dynamically may not be credited. Neither limit
can make the check pass something that is *wrong* — only quieter.

## D-124 The command payloads are checked against what the command layer reads (2026-09-26)

D-123's defect — the driver sending `error_class` on every `record_attempt` while nothing ever stored it — was
found by a probe that needed the field, which is luck, not a check. It is a mechanical property though: a
control command is a method plus a payload, and a payload field the command layer never reads is dropped in
silence. So it has a detector now:

    python3 review/command_params.py
    22 commands, 67 payload fields, every one read by core/src/v2/control.rs

It reads every `command(…, "method", json!({…}))` call in `engine/src`, takes each payload's top-level keys, and
compares them with the keys the command layer reads anywhere in `control.rs` (a field read by a helper counts —
`record_attempt`'s `publish` is read by `publish_list`, which is why a per-handler scope would have produced a
false positive here). It runs inside `make hygiene`, so the next field that is sent and never read fails
`make check` instead of waiting for someone to need it.

Control: with `params["error_class"]` removed again (the pre-D-123 shape) it reports exactly
`record_attempt: the command layer reads no ['error_class']`, listing all four driver call sites — the same four
that were found by hand. The current tree has zero findings, which is the honest result of the sweep: D-123 was
the only such field.

Ceiling: the payload is parsed by brace and key matching, not by a Rust parser, so a key built at runtime is
invisible; fields sent by the *clients* (`exec`, the TUI) rather than by the engine are out of scope, because
the scan only reads `engine/src`. Both limits make it quieter than a full check — every finding it does report
is a field the engine takes the trouble to fill.

## D-123 A19's whole path over a real socket, and the failure class nothing stored (2026-09-26)

A19's two halves existed separately: `providers_fake` classifies a truncated stream per protocol against
in-process fake servers, and D-117 tests the driver's retry with a scripted provider. Neither drove the engine's
own HTTP/SSE stack over a socket. `python3 review/dogfood/truncation.py` does, against a local
chat-completions server — so it needs **no credentials and no network** — and runs two scenarios through the
real binary:

* **truncated before any visible text** (a delta carrying only a tool-call id, then the connection closes): the
  attempt is transient, the driver retries inside the turn, the second response completes with a `finish` call,
  and the run is exit 0 with the goal `SUCCEEDED`. Measured: the server saw exactly **two** requests, and the
  session's attempts table holds `FAILED` with **no usage** and class `Transient` next to the priced `COMPLETE`
  attempt; the truncated attempt's call id never reaches the conversation.
* **truncated after visible text**: the attempt is *permanent* (a retry could duplicate text the user already
  saw). Measured: exit **1**, `failure` beginning `permanent model error: model stream ended before completion;
  partial …`, exactly **one** request, one `FAILED` attempt with class `Permanent`, and the partial text absent
  from the conversation.

Writing it found a defect of the "declared but not served" family, this time in the database rather than in the
config: the driver sends `error_class` in every `record_attempt` (four call sites) and `attempts` has the column
(`store.rs`), but `control::record_attempt` never wrote it — so every attempt row said `NULL`, and the one field
that explains *why* a request was retried was silently dropped. The probe's own assertion is what exposed it
(its classification checks failed against rows that were all `NULL`).

Fix: `record_attempt` reads `params["error_class"]` and inserts it; the core test
`attempts_for_closed_requests_are_refused_and_unknown_usage_is_visible` now asserts the round-trip. Control: with
the column dropped again from the INSERT, that test fails at `the attempted failure class is dropped: None`,
and the probe's classification assertions fail with it.

Evidence: the two scenarios above (2026-09-26, ~6 s, credential-free); the control; and `make check` green.

Ceiling: the probe covers the two classification branches; the *budget* half (retries exhausted, D-117) and a real
remote truncation stay where they are. The permanent branch's fixture sends visible text before closing, which is
the shape `providers_fake` established; a truncation that lands *between* two tool-call deltas is that same
fixture's neighbour and is classified by the same rule.

## D-122 The positive verification targets could not fail (2026-09-26)

The formal-verification targets carry the "what can be machine-checked must be machine-checked" half of the
project's guard rails, so it matters that they can actually fail. Three of them could not:

    verify-model / verify-model-all / verify-model-wide:
        java … tlc2.TLC … | grep -E "No error|violation|violated|states generated"
    verify-kani:
        cargo kani --lib | grep -E "VERIFICATION|Complete -|failed"

A recipe's status is its last command's, and that is the `grep` — whose pattern *includes the failure words*
(`violation`, `violated`, `failed`). So the target passes exactly when TLC or Kani reports the thing it exists to
catch. Proved by pointing the shipped shape at a configuration that must be refuted (one of the counterexample
configs, `MC_authority_badview.cfg`):

    == MC_authority_badview.cfg (expect a violation) ==
    Error: Temporal properties were violated.
    165 states generated, 62 distinct states found, 46 states left on queue.
    exit=0        # the untouched target "passed"

The same run with the assertion added exits 2 with `MC_authority_badview.cfg did not verify`. (The
`verify-model-counterexamples` target was already correct: it *requires* the violation line and fails when a
control verifies instead — which is why the negative controls could never have hidden this.)

All four positive targets now require their success marker — `No error has been found` per TLC configuration,
and a Kani summary matching `Complete - <n≥1> successfully verified harnesses, 0 failures` — while still printing
the grep lines so a failure is visible as well as fatal.

Evidence: the control above (shipped shape exit 0 against a refuted config, asserted shape exit 2); the real
targets re-run on this tree (`make verify-model-all`: 11 configurations, every one "No error has been found";
`make verify-model-counterexamples`: 10 controls, each refuted). The rest of the Makefile was swept for the same shape: every other recipe asserts its own
failure path (`sha256sum -c`, the daemon-count guard, the three detectors, `sh -n install.sh`), and
`language-check` is the deliberate inverse — its `git grep` *matching* is the failure, so matching is what
exits 1.

Ceiling: `verify-kani` needs the Kani toolchain, which is **not installed in this environment**, so its fixed
assertion is unexercised here. The proof itself does not need a fresh run to stand: its subject is unchanged since
the verified commit (`git log -L :page_span:core/src/kernel/types.rs` is a single commit, and the harness crate
changed only in comments since), so the recorded result carries over by identity — a machine with the toolchain can
re-run it (`cargo install --locked kani-verifier && cargo kani setup`, the command the Makefile prints).
`verification/REPORT.md` records that split explicitly rather than implying the proof was re-checked.

## D-121 Two tests were skipping silently, and nothing looked (2026-09-26)

D-113 fixed the tests that *said* they skipped and the one that failed on a sandbox-less machine; D-114 gave every
sandbox-dependent test a fail-closed branch. Two sites were still invisible because they did neither: they
`return`ed from a capability guard with no message at all.

    engine/src/tools.rs  cancelled_shell_keeps_partial_output_and_its_artifact
        if !sandbox_usable() { return; }
    engine/src/tools.rs  sandbox_builds_with_the_host_toolchain
        if !sandbox_usable() || toolchain_mounts().is_empty() || which("cargo").is_none() { return; }

So on the GitHub runner (no bubblewrap) both contributed exactly nothing, and nothing in any log said so — the CI
comment even promises that such tests "print 'skipped: bwrap is unavailable'". The second one hid a *second*
condition as well: a missing `cargo`, or a machine that exposes no host toolchain to mount, looked the same as a
broken sandbox.

Both now say what is missing, per condition, and keep the claim honest: the cancelled-sandbox test cannot be
observed without a running sandbox (its fail-closed half is
`cli::an_unisolated_shell_refuses_instead_of_running_on_the_host`, D-113), and the toolchain test only ever ran
where a host toolchain exists. A skip is fine; a *silent* one is not.

The class has a detector now, because it is mechanical and it has now produced three rounds of findings:
`python3 review/silent_skips.py` reports a `#[test]`/`#[tokio::test]` function containing an `if ... {` block whose
body is only `return;` — after the fix, zero findings. It is narrow on purpose: a guard inside the test's *own*
helper (a polling `async fn` whose timeout path panics, or the writer loop of a fake server handed to
`tokio::spawn`) is not a skip and is not reported, which is what the four remaining `return`s in the suite are. It
runs inside `make hygiene`.

Control: deleting the message from the cancelled-sandbox guard makes the detector fail with that file and test
name; the nested-helper cases stay quiet in the same run.

Ceiling: the rule sees `return;`. A skip written as `for ... { continue; }`, or a test that quietly stops
asserting anything, is outside it — the first is rare, the second is what review is for.

## D-120 The budget's live half needs no credentials (2026-09-26)

A18's ceiling was verified in-process and by the CLI tests, and D-97 recorded a live run — but as a *manual*
run, so nothing re-runnable stood behind the claim that a user's ceiling is enforced before any money is spent.
It is now a probe, `python3 review/dogfood/budget.py`, and its most useful property is what it does *not* need:

* **no credentials and no network.** The refusal happens before the provider is asked for anything
  (§7's admission gate), so a dummy key in the environment is enough — the probe can be re-run on any machine,
  including one with no API key and no route out. (Its sibling `deadline.py` cannot make that promise: it needs a
  real first turn.)
* it asserts the *whole* shape in one 1-second run: `exec` exits **1** with `end=failed` and a `failure` naming
  the ceiling (`goal goal-s-main budget exceeded: known 0 + reserved 0 + est 2228 > max 1000`), the session
  holds **zero** `model_requests` rows (nothing was spent), a `budget_refused` event carries the same arithmetic
  (`known`/`reserved`/`est`/`max`), the leader is `PARKED` with that sentence, the goal carries
  `max_total_tokens`, and the attached `--check` never ran (D-96: a turn that never started verifies nothing).

Measured 2026-09-26: three runs green (two at `max_total_tokens = 1000`, one at `500`, which correctly reports
`est 2228 > max 500`), 0.6–1.0 s each.

Ceiling: the probe covers the *refusal* half — the ceiling is below one request, so the run is over before any
model call. The other half of A18 ("the ceiling bounds a session that does spend") stays with the in-process and
CLI tests and D-97's recorded live run, because observing it honestly needs real tokens spent.

## D-119 A09's live half: the crash that leaves nothing verifiable (2026-09-26)

`crash.py` is the *recoverable* crash (A08/A11): the daemon dies, the runner survives, its receipt is consumed
once. A09 is the other half — the crash after which nothing can be verified — and it is a promise about honesty
rather than about recovery, so it gets its own probe, `python3 review/dogfood/unknown_outcome.py`:

1. the Leader hires one worker (the product path) and the user grants it `shell@workspace`
   (`teamagents authority grant`; a spawned worker holds nothing of its own, §5.1/D-61);
2. the user asks for a delegated task whose text is exactly one shell call, and waits for the moment A09 is
   about: **the runner's own journal says the command started** and the delegated task is `RUNNING`;
3. the probe kills the **runner** (so no terminal receipt can be written) and then the **daemon**, and starts a
   cold daemon: recovery respawns a runner over the same job directory, which marks the journal `OUTCOME_UNKNOWN`
   because a runner restarted over a non-terminal state cannot know whether the old child produced effects;
4. the artifacts decide: `runs.log` holds exactly **one** line (the command ran once, never replayed), the
   worker's operation is `OUTCOME_UNKNOWN` with receipt class `outcome_unknown`, the task is `BLOCKED` with
   exactly one `task_blocked` notification, and no goal claims success. The orphaned command (its process group
   outlives the runner, D-41) is stopped by pid at the end — the user's own lever, as the guide says.

Measured 2026-09-26 (DeepSeek Flash, native window): two consecutive runs green, ~10 s each.

Writing it took three predicates, and the two wrong ones are the record's real content:

* "the worker is in `TOOLS_PENDING` and `runs.log` exists" — a model can *write* the side-effect file itself
  (one run did exactly that: `write_file runs.log`), so the probe killed nothing and had no crash to recover
  from. The side effect is not a witness when the agent can author it.
* "a shell operation is `DISPATCH_COMMITTED`/`RUNNING`" — that only says the driver committed the dispatch. A
  runner killed before it accepts the GO leaves a `READY` journal, and recovery then legitimately starts the
  command **once** (§6.2: a READY journal means GO was not accepted). Two runs measured a successful replay —
  A08's scenario — while the probe believed it was testing A09.
* the shipped predicate: the **runner's journal** is `START_ACCEPTED`/`RUNNING` **and** the delegated task is
  `RUNNING`. The journal is the state machine's own statement that the command started, which is exactly the
  fact A09's claim is about; the operation row is a different fact (dispatch was committed).

This is the same lesson as D-83/D-94/D-103/D-105/D-111/D-117 ("wait for the effect you assert about"), one level
up: here the wrong witness was not too early in *time* but too early in the *state machine*, and it silently
switched which scenario was under test.

## D-118 A02's ring is verified with a real model (2026-09-26)

A02 ("A→B→C→A communication") had two kinds of evidence: the control plane
(`control::messages_flow_across_an_authorized_ring`) and the authority surface (D-61). Neither shows what a user
does with a team — ask a Leader to hire people and then let the members talk — so the ring now has a live probe,
`python3 review/dogfood/team_ring.py`:

1. the user asks the Leader to hire two workers (the product path; the probe reads the member ids from the
   session, it does not name them);
2. a spawned worker holds no authority of its own (§5.1/D-61), so the user grants both `message@session` through
   `teamagents authority grant` — the permission step is part of the ring, not scenery;
3. the user submits one instruction to B and one to C through the daemon's `submit_input` (the TUI's own call),
   each asking for exactly one `send`;
4. the artifact is the **recipient's context**: the delivered envelope is rendered by the runtime as
   `[message from <sender>] <text>`, and the probe additionally asserts the two `message_sent` events exist with
   the expected sender and recipient. Nothing is read out of a model's prose.

Measured (2026-09-26, DeepSeek Flash, native window): the ring closed in **23.5 s** and **14.2 s** in two runs.
The first run also shows the members talking beyond the minimum (B answered A as well, and one `send` went to
itself); the probe asserts the ring's hops rather than silence, which is the honest scope. A hop whose turn
produced no `send` is re-asked up to three times — the claim is about delivery, not about one turn's obedience.

Two things were learned by writing it. A **delivered message is a `user`-kind context entry**, not a
`message`-kind one (the envelope is applied to the recipient's context, §5.3), which is why the probe's wait
looks for the rendered `[message from …]` line and asserts the sender with it. And a `message_sent` event
carries the **recipient in the event's scope**, not in its payload (`event(…, "message_sent", recipient, …)`); a
probe that reads a payload field there sees nothing.

Ceiling: the probe drives one hop at a time as the user, so it proves that the ring *can* close through the
product surfaces and that delivery is attributed — not that a single turn orchestrates a three-hop ring on its
own (that would be a much flakier claim, and the members' extra sends in the first run are the reason).

## D-117 The retry path had no test, and its wait was uninterruptible (2026-09-26)

A19's evidence was the provider edge: `providers_fake` classes a truncated stream before visible output as
transient and after it as permanent, per protocol, and `providers_stall` drives keep-alive sockets. The *driver's*
half of that contract — the retry that happens inside the turn, its budget, and what a turn looks like when the
retries run out — had no test at all, and could not have one: the scripted provider of `engine/tests/v2_driver.rs`
could express a permanent, an interrupted, a context-overflow and a hang, but **not a transient failure**.

With `Step::Transient` / `Step::TransientAfter` (the latter carries the provider's own `Retry-After`) two tests
pin the shape:

* `a_transient_model_failure_is_retried_inside_the_turn`: attempt 1 fails transiently, attempt 2 completes, the
  goal settles `SUCCEEDED`, the events carry `FAILED` then `COMPLETE`, the `attempts` table has two rows — the
  failed one with **no** usage, the complete one priced — and the goal's `known_usage` counts only the complete
  attempt while `unknown_usage` stays 0. (D-123 later made the *class* of that failed row assertable: the driver had always sent
  `error_class` and the control plane had never stored it.) That is §6.3's "double billing is possible, and the runtime does not
  pretend to know what the lost attempt cost", asserted instead of described.
* `transient_retries_exhausted_parks_with_the_reason`: three transient failures (the budget is `max_retries = 2`),
  then the turn ends with the documented sentence — `transient retries exhausted: boom three`, the *last* failure
  — the instance parks once, no goal settles, and no reply is fabricated.

The third test was written to *measure* the wait and found a defect instead:
`a_shutdown_does_not_wait_out_a_retry_backoff`. The wait selected on a `Cancel` token nobody can fire while it
runs (the driver clears `shared.cancel_attempt` right after the provider call returns, and the only caller of
`cancel_turn` is parked under D-63), so a provider answering `Retry-After: 30 s` made a driver stop take 30 s.
Measured before the fix — the in-process test failed after 30.21 s, and the daemon-level one
(`v2_daemon::a_daemon_stop_does_not_wait_out_a_provider_backoff`, which goes through the supervisor's
`shutdown_shared` and its "await every driver task") failed at `the daemon stop waited out the provider backoff
(29.984226755s)`. That is what a `Ctrl-C` on the daemon did: print `stopping…` and sit there for as long as the
provider asked.

The wait now also selects on the driver's own shutdown (`Driver::wait_for_shutdown`, the same wake-plus-flag
convention every other driver loop uses), so a stop ends the turn at the loop's next shutdown check instead of
sitting on the backoff. The daemon test then finishes in 0.39 s, and removing the arm reproduces the 29.98 s hang
(the recorded control).

Evidence: four new tests; three gate conditions green at 353 tests (core 100 / engine 220 / tui 33 — the two
sandbox-less conditions run the retry tests too, since they need no shell); the control above. The retry tests
are also what would fail if the classification were wrong: a permanent class produces one attempt row and the
`permanent model error` sentence, not two rows and `transient retries exhausted`.

Ceiling: the backoff itself is unchanged (500 ms shifted by attempt, or the provider's `Retry-After`, capped at
8 s / 30 s), and **no surface cancels one turn while it waits** — that is still D-63's open question; what ends
the wait now is the runtime stopping, not a user interrupt. `max_retries` is a driver-config knob (the CLI passes
2), not user config.

## D-116 A running command cost ~4 % of a core, and nobody had measured it (2026-09-26)

Every command a member runs pays two cadences for as long as it runs: the runner's tick (how often it looks at
its child) and the driver's `status` poll (one socket round trip per poll, §6.3). Neither had been measured, and
this product's stated priority is long-horizon stability — a team of members building in parallel pays whatever
the constants happen to be. Measured on this machine (`python3 review/runner_cost.py`: one `sleep 45` command in
a runner the probe starts itself, 10 s windows, `/proc/<pid>/stat`):

| what | before | after |
|---|---|---|
| the runner's tick alone (no poller) | 1.80 % of a core (10 ms tick; three samples 1.80/1.80/1.80) | 0.47–0.50 % (50 ms tick) |
| the runner serving the driver's polls | 2.30–2.40 % (25 ms cadence, ~300 requests / 10 s) | 0.70–0.90 % (50 ms for the first second, 250 ms after) |
| **per in-flight command** | **~4.2 %** | **~1.2 %** |

Each round trip costs the runner ~0.8 ms of CPU (803 µs at the 25 ms cadence, 888 µs at 50 ms — the same work,
fewer times), which is why the poll dominated: 40 requests/s is 40 journal reads, parses and replies.

The change is three named constants and no behaviour change beyond latency: `jobs::runner::TICK` = 50 ms (with
`CANCEL_ESCALATION_MS` named beside it), and the driver's `JOB_POLL` = 50 ms for the first second, then
`JOB_POLL_IDLE` = 250 ms (`JOB_POLL_PATIENCE` = 1 s). What the cadences buy is bounded and unchanged in kind:

* a command's exit reaches the model within one tick (≤50 ms) or one poll (≤250 ms after the first second — a
  quick command finishes inside the first second, where the finer cadence still applies),
* a past deadline is enforced within one tick,
* TERM→KILL lands within `CANCEL_ESCALATION_MS` plus one tick (≤550 ms),
* a user's cancel (`instances terminate`, the TUI's key) is noticed within 250 ms.

Both bounds are compile-time invariants now (`jobs::tests::the_runner_tick_trades_cpu_for_a_bounded_latency` and
`v2::driver::cadence::the_job_poll_is_coarse_but_prompt` use `const { assert!(…) }`, so a wrong constant fails
the build instead of a test run).

Evidence: the probe above (re-runnable, no model, no daemon); three gate conditions green at 349 tests
(core 100 / engine 216 / tui 33); and the live stop lever re-run after the change
(`python3 review/dogfood/cancel.py --state-dir /tmp/ta-cancel-cpu2`: the effect stopped 4.0 s after the lever,
the receipt class was `cancelled`) — the coarser cadence delays only the *notice*, by at most 250 ms.

Ceiling: the cadences are compile-time constants, not config, so a user cannot trade latency for CPU. A command
that runs for hours is still ticked 20×/s and polled 4×/s, so ~1 % of a core remains per in-flight command; the
way below it is a push channel from the runner (new protocol, a design change) or a longer idle cadence, not a
finer poll. The numbers are this machine's, which is why the probe prints them instead of asserting thresholds.

## D-115 The isolation verdict carries its reason (2026-09-26)

D-114 gave one shared predicate, but only its boolean half was used: `doctor` printed a fixed sentence for the
"installed but blocked" case, and the MCP workspace refusal said `IsolationUnavailable: MCP workspace execution
requires bwrap` — wrong on exactly the machine that motivated D-114, where bwrap *is* installed and the machine
blocks it. The probe now returns the reason together with the verdict (`tools::sandbox_state()`), and its three
readers use it:

* `doctor` puts bwrap's own last line in its FAIL row: `[FAIL] bubblewrap isolation  bwrap is installed but the
  isolation probe failed: bwrap: setting up uid map: Permission denied; check that the system allows
  unprivileged user namespaces`. A row is one line, so the machine's words are the actionable part; the generic
  "the sandbox failed to start" sentence above them only repeats what the user already sees.
* the MCP workspace path refuses with it too: `IsolationUnavailable: MCP workspace execution requires a working
  sandbox: <reason>`.
* the two `v2_mcp` sandbox tests now assert the same sentence on **both** sandbox-less machines, and where bwrap
  is installed that the refusal repeats bwrap's own words (`Permission denied`) — an assertion that was
  impossible before, because the message never carried them.

The first full three-condition run (`make check`, `make check-nobwrap`, `make check-broken-sandbox`) also lost one
fixture race, and it is the D-83/D-94/D-103/D-105/D-111 shape once more, one level down:
`startup_timeout_s_bounds_a_silent_handshake` (D-108) asserted the failed handshake had reaped the fixture server
by reading the pid the fixture writes as its *first* statement — inside a 1 s `startup_timeout_s`. On a loaded
machine python's interpreter start can exceed that bound, so the driver reaps the server before it writes
anything and the test fails on "the fixture server wrote its pid within 30 s" (after waiting the full 30 s under
the stub condition, 5 s under the other two). The bound is 5 s now: still 12× below the 60 s default and 6× below
the fixture's own 30 s stall, so the claim ("the configured bound is applied, not the default") is unchanged
while the fixture has room to exist. The pre-fix control still holds — the 60 s default waits the 30 s stall out
and boots.

Evidence: three conditions green at 347 tests each (core 100 / engine 214 / tui 33); the doctor rows quoted above
are the literal output of `teamagents doctor` with `/tmp/broken-farm` on `PATH` (`review/nobwrap_path.py
--stub-bwrap`); the MCP assertions live in `v2_mcp::mcp_workspace_execution_is_sandboxed` and
`mcp_workspace_network_follows_the_config_key`.

Ceiling: the reason is the *last non-empty line* of the probe's failure — bwrap's own message for the blocked
case, the generic isolation sentence when bwrap printed nothing — and the verdict itself is still a
first-observation snapshot, cached for the process (D-114's ceiling).

## D-114 "bwrap is installed" is not the capability (2026-09-26)

D-113's ceiling was the second sandbox-less machine, and it is a machine a user is likely to have: **bwrap
installed and unable to create a namespace**. Ubuntu 23.10+/24.04 restricts unprivileged user namespaces by
default (AppArmor), so the binary dies with `bwrap: setting up uid map: Permission denied`; a locked-down
container does the same. `bwrap_available()` is a `PATH` lookup, so it answers `true` there and the tests took
their *positive* branch: seven sites failed (not skipped), each at an assertion that assumes a working sandbox —

    python3 review/nobwrap_path.py /tmp/broken-farm --stub-bwrap   # every tool, plus a bwrap stub that prints
                                                                   # the Ubuntu line and exits 1
    env PATH=/tmp/broken-farm <cli test> --test-threads=1 an_unisolated_shell_refuses_instead_of_running_on_the_host
    # FAILED: "the control check runs" — with `… "ok":false, "exit_code":-1, "error":"IsolationUnavailable: the
    # sandbox failed to start; the command may not have run. … bwrap: setting up uid map: Permission denied"`

and the same shape in `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check`, both `v2_daemon`
acceptance-check tests, both `v2_mcp` sandbox tests, `exec`'s unit test and
`tools::workspace_paths_stay_inside_root`. The *product* was right in every one of those runs and said so:
the sandbox refuses with bwrap's own words and never falls back to the host. What was wrong was the *capability
predicate* — `doctor` already probed here ("not just 'is it installed': run a probe so a broken userns/kernel
setup is caught here instead of at the first shell call"), and the tests used the `PATH` lookup instead.

The probe is shared now: `tools::sandbox_state()` (bwrap in `PATH` **and** a trivial sandboxed command
succeeds; cached for the process life, and it carries the reason — D-115) is what `doctor` reports and what the
tests branch on. Every
sandbox-dependent test now has three cases — working sandbox, no bwrap, bwrap that cannot start — and asserts
the fail-closed half in the last two, so the A14 claim ("a command that cannot be sandboxed is refused, never
run on the host") is asserted on all three. The execution path is deliberately unchanged: it keeps using
`bwrap_available()` and reports bwrap's own failure, which is more useful to a user than "not installed".

Evidence:

    make check-broken-sandbox   # the gate in that condition: 347 tests, core 100 / engine 214 / tui 33, green
    make check-nobwrap          # the CI condition (D-113): 347 tests, green
    ./engine/target/debug/teamagents doctor
    #   working machine: [ok  ] bubblewrap isolation  isolation probe passed: system files visible, …
    #   stub farm:       [FAIL] bubblewrap isolation  bwrap is installed but the isolation probe failed:
    #                                                  bwrap: setting up uid map: Permission denied; check that the
    #                                                  system allows unprivileged user namespaces

Control for the branch selection: with the stub farm in `PATH` and a panic injected into the no-sandbox branch,
`cli::an_unisolated_shell_refuses_instead_of_running_on_the_host` panics there — the branch is driven by
`sandbox_usable()`, not by `PATH`. Before the change the same farm failed that test at its positive assertion.
`tools::workspace_paths_stay_inside_root` also gained a case: `glob` prefers `rg` *inside* the sandbox, so a
sandbox that cannot start refuses instead of globbing differently, while `ls` is native and keeps working —
both are asserted.

Ceiling: the probe runs once per process and is cached, so a sandbox that becomes unusable *during* a long
session (an AppArmor/SELinux reload, exhausted pids) still reads as usable; the failure then shows up per
command as the `isolation` class, which is the honest place for it. The three targets run the gate three times
over (~3 minutes each on this machine), and `--stub-bwrap` is a stub, not the real AppArmor mechanism: it
reproduces the *shape* a blocked bwrap produces (`bwrap: …` on stderr, non-zero exit), which is what the code
classifies.

## D-113 The gate could not be green where CI runs it (2026-09-26)

`docs/ACCEPTANCE.md` opens with "`make check` is green … preconditions for every item below", and the GitHub
workflow runs exactly that on a runner whose kernel forbids unprivileged user namespaces and which has no
bubblewrap installed — the workflow comment said so, and it is now checked outside this repository too: the
published `actions/runner-images` package lists for Ubuntu 24.04 and 22.04 (`ubuntu-latest` is 24.04) contain
no `bubblewrap` entry, so the runner is the "bwrap absent" machine this entry fixes. That machine could not pass it. `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check`
asserted that a `--check` command runs and reports `ok` — true only where a sandbox exists — and it has no
capability guard, so on the runner the run reports the check as refused and the test fails:

    env PATH=<a PATH with every tool except bwrap> <cli test binary> --exact \
        exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check
    # panicked at tests/cli.rs:426: check 1: FAILED (exit -1)  echo accepted

Three more tests took the other way out and skipped whole: `cli::an_unisolated_shell_refuses_instead_of_running_on_the_host`
(A14's own test), `v2_daemon::headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code`,
`v2_daemon::a_failing_acceptance_command_fails_the_run` and
`exec::tests::acceptance_commands_run_in_order_and_stop_at_the_first_failure` each printed
"skipped: bwrap is unavailable" and returned — so on the machine that runs every push, the A14 claim ("a command
that cannot be sandboxed is refused, never run on the host") and the acceptance-check ledger were **unchecked**,
and one test simply failed.

All four now assert the half that *is* observable without a sandbox, which is the fail-closed half the product
promises: the check is reported `ok: false` with `exit_code: -1`, its `error` names the isolation
(`IsolationUnavailable`), no command output appears (it never ran), the list stops at the first refusal, and the
run is an honest failure (exit 1) rather than a pass over an unchecked goal. Where bubblewrap exists the
positive half is asserted as before, including A14's control (the same command really runs and leaves its
trace). Nothing skips any more; no test lost an assertion.

The condition is now reproducible instead of implicit:

    make check-nobwrap      # review/nobwrap_path.py builds a PATH with every tool except bwrap, then runs make check

Measured: with that PATH the whole engine suite is green (214 tests, 15 binaries) where
`exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check` failed before; core (100) and tui (33) are green
there too. `review/nobwrap_path.py --verify` asserts the farm really has no bwrap and does have `sh`, `python3`,
`cargo` and `make`, so the target cannot silently test the wrong PATH.

Ceiling: a CI machine *with* a broken sandbox (bwrap installed but unable to create a namespace) is neither
condition: `bwrap_available()` only looks at `PATH`, so there the positive half is attempted and fails, and the
refusal branch is not taken. That machine fails loudly rather than lying, which is the right side to be on, but
it is not covered by either branch here.

## D-112 Nothing ever stopped a shell runner (2026-09-26)

§6.2 gives "one controlled runner per *active* command", A12's own test ends its runner with
`jobs::client::shutdown`, and the verb has existed as long as the runner has — but **the product never called
it**. Every shell command and every acceptance check therefore left a detached `jobs-runner` process behind:
reparented to init (measured: `PPID 1`), holding its job directory under `<state root>/jobs/<op>`, ticking on a
10 ms interval for the life of the machine. Measured on one settled job (`state: SUCCEEDED`, `exit_code: 0`,
`finished_ms` set, one start): **11 CPU ticks in 6 s at rest**, ~1.8 % of a core per idle runner. A single
`engine` test suite left **25** of them; a long session would leave one per command, and §6.4 says a daemon
shutdown deliberately does not reap them.

The fix goes in the one place that builds a job directory (`driver.rs`, `execute_shell`): once the terminal
receipt is committed, a job with a *known* outcome sends `client::shutdown`. It is deliberately the last act
after `complete_op`, so the outcome is durable before the runner goes; the journal on disk stays the recoverable
record and §6.3's recovery respawns a runner over the same directory when it needs a status or a cancel. It
stops the *runner*, never a service the command left behind — A12's own test
(`a_successful_commands_service_outlives_the_job`) asserts the `sleep 300` it spawned survives the runner's
shutdown, and D-41 makes such a service explicit-only cleanup.

An unknown outcome keeps its runner on purpose: that is the state where §6.2/§6.4 want a live partner for
identity-verified cancellation and verification. After the fix, a whole engine suite (15 test binaries) leaves
exactly one runner, and its journal reads `OUTCOME_UNKNOWN` — the crash fixture's, the one that is supposed to
stay.

Evidence (all commands re-runnable):

    cargo test --offline --manifest-path engine/Cargo.toml --test v2_driver a_settled_shell_job_leaves_no_runner_behind
    #   the new test: the journal is still SUCCEEDED on disk and `client::status(job_dir)` fails, i.e. no runner answers
    #   pre-fix control: with the shutdown removed it fails at
    #   "no runner answers for a settled job (a live one would reply to `status`)"
    #   suite A/B, counting `jobs-runner` processes by the state in their journal:
    #     before: 25 (v2_driver suite) — after: 0, with the single OUTCOME_UNKNOWN one still there
    # live, real model: python3 review/dogfood/checks.py --provider deepseek (a goal blocked by an impossible
    #   configured check after three rounds) ends with 0 jobs-runner processes and 0 daemons

Re-verified against the real product path after the change, not only in-process: `review/dogfood/job_identity.py` (A15/A10: journal identity re-derived from `/proc` while a command ran, duplicate GO kept `starts` at 1, a guessed token refused) and `review/dogfood/cancel.py` (A13: the effect stopped 1.5 s after the lever, receipt class `cancelled`, **0 runners and 0 daemons** left at the end) both pass with the retirement in place, and `review/dogfood/checks.py --provider deepseek` ends with 0 runners.

The measurement that found it was the D-111 gate, which counts `teamagents daemon` processes: its first version
counted every `teamagents` process and failed with "27 daemon(s) behind" — those 27 were these runners. D-111's
entry records the correction to the *gate*; this entry records the *defect* the correction exposed, which is why
the two are separate.

Ceiling: an `OUTCOME_UNKNOWN` job keeps its runner (and its 10 ms tick) until the state root is retired.
Resolving such an operation (the user cancels or retries the parked task) is a database action, and the runner
is only reachable over its socket, so nothing retires it today — the supervisor's recovery pass would respawn
one if it needed it. A follow-up could send the shutdown when the operation is resolved; deleting the state
root removes it either way. `jobs_runner`'s own tests still manage their runners by hand (they test the verb
itself).

## D-111 The suite leaked daemons, and now it cannot (2026-09-26)

The A14 test (`cli::an_unisolated_shell_refuses_instead_of_running_on_the_host`) drives two `teamagents exec`
runs, and `exec` *detaches* the daemon it starts — that is its contract, the session outlives the client. Both
runs therefore left a daemon running under `/tmp/ta-isolation-<pid>/root` and `…/root-control`, and because the
daemons kept those roots busy the test's `let _ = std::fs::remove_dir_all(&root)` left the directories behind as
well. The file already states the rule, in the `Daemon` guard at the top: "a leaked daemon would keep running
(and hold a coordinator lock) for the rest of the suite". Every other test in it either wraps its child in
`Daemon` or stops the detached daemon with `pkill`; this one did neither.

Measured with only that test selected:

    cargo test --offline --manifest-path engine/Cargo.toml --test cli an_unisolated_shell_refuses_instead_of_running_on_the_host
    ps -eo pid,etime,args | grep "[t]eamagents daemon"   # two processes, alive after the test binary exited
    ls -d /tmp/ta-isolation-*                           # their state roots, still there

The test now stops both daemons and *asserts* they are gone (a bounded `pgrep` poll, then the state root is
removed with `expect` instead of `let _`). Pre-fix control: with the stop disabled the test fails at the new
assertion and prints the pids it found (`the daemons this test started are gone: 111 137`).

The class also gets a gate, because a per-test rule with nothing behind it is what produced this in the first
place: `make test` counts `teamagents daemon` processes before and after the three suites and fails when the
count grows, naming the rule and this entry.

The count is deliberately about *daemons*: the first version counted every `teamagents` process and failed at
once with "27 daemon(s) behind", all of them `jobs-runner` children. Those are the A12 shape — a command's
service outlives the job — they linger for tens of seconds and exit on their own, so counting them would have
made the gate cry wolf. The control below proves it still fires for a real leak:

    make test                                      # 0 daemons before, 0 after
    make -f /tmp/Makefile.leak test-leak-probe     # "the suite left 1 daemon(s) behind (before: 0, after: 1)"

**Count by subcommand, not by name** (measured again 2026-09-26): `ps -eo comm | grep -cx teamagents` counts
every process of that binary and therefore reports **1** on a clean tree. The one it finds is the A08 crash
test's `OUTCOME_UNKNOWN` runner (`v2_driver::tool_result_is_reused_after_crash_not_reexecuted`, whose state
root is `teamagents-v2-driver-crash-runner-*`): it stayed alive through a whole `make check` and for another
90 s of polling, then exited by itself — longer than the "tens of seconds" this entry first assumed, and still
the A12 shape rather than a leak. What answers "did the suite leave a daemon" is the guard's own
predicate, `ps -eo comm,args | awk '$1=="teamagents" && $3=="daemon"' | wc -l` → 0.

Ceiling: the count is a delta, so a daemon the developer already had running is not blamed; a *daemon* a test
forgot to stop is caught, a leaked `jobs-runner` is not (it exits by itself, and nothing here proves how long
that takes under load — the runner's lifetime rule is A12's). A test that forgets its own stop still leaks
within its own state root; the gate only notices the daemon.

## D-110 Nothing checked the documentation's citations (2026-09-26)

`docs/ACCEPTANCE.md` is the evidence ledger and `review/README.md` is its index, so their citations *are* the
re-runnable commands: `` `v2_driver::end_to_end_shell_then_finish` ``, `` `review/dogfood/crash.py` ``. A
citation that names a renamed test, a removed file or a path that never existed makes the claim it supports
unverifiable while still reading as evidence — and nothing read them. The same class produced D-53 (documented
claims corrected to the code), D-78/D-86 (public items nothing calls) and D-102 (a config key nothing reads),
every one of them found by hand.

The audit is now a command: `python3 review/citations.py` reads the tracked markdown (docs, review, README,
AGENTS) and resolves three kinds of backticked citation against the tree — a qualified name in the a::b shape
(against every test name, every `fn`/`const`/`struct`/… name and every module file), a repository-looking path
(under `docs/`, `engine/`, `tui/`, `core/`, `review/`, `verification/`, `examples/`), and a `.rs` basename
(also in Rust sources, where it is the only rule). First run: 275 citations, 0 unexplained, 7 recorded as
removed. It runs inside `make hygiene`, so `make check` fails on the next citation that stops resolving.

The "recorded as removed" half is deliberate: this file keeps tables of items the dead-code sweeps deleted, and
a citation inside such a table *is* how the removal is recorded. The rule is mechanical — the citing line, or
the header row of the markdown table it sits in, must contain a marker word (`deleted`, `removed`, `no longer
exists`, `404`, …). A first implementation joined every row of the enclosing table into that context instead of
its header, which let a `removed` in one row of `docs/ACCEPTANCE.md`'s single A-matrix excuse citations in any
row below it; the negative control caught it (renaming a test in the code left the ledger citation unflagged).

Pre-fix controls: a line added to `docs/ACCEPTANCE.md` citing the test
`v2_driver::no_such_test_exists_at_all` and the suite `engine/tests/ghost_suite.rs`, both do not exist,
produces exactly two findings and exit 1 (and the two findings this audit
found in the tree were fixed rather than allowed: `docs/INSTALL.md`'s phantom `config.example.toml` path and a
comment in `engine/tests/v2_mcp.rs` citing the removed pre-v2 `mcp_tools.rs`); renaming
`mcp_workspace_execution_is_sandboxed` in `engine/tests/v2_mcp.rs` makes the ledger's own citation fail, so the
check tracks renames and not just the absence of a string.

    python3 review/citations.py
    make hygiene     # ... which runs it

Limits: bare basenames (`report.json`, `notify.sh`, `INPUTS.md`) are runtime artifacts, user-written files and
often prose, so they are out of scope; a citation of *another* project's path cannot be resolved by this tree
at all — D-109 is exactly one of those, found by hand — and the two upstream rows in
`docs/PRODUCT-COMPARISON.md` now mark the upstream prefix (`packages/…`) so they are not mistaken for ours.

## D-109 The Pi row claimed things the upstream project does not have (2026-09-26)

`docs/PRODUCT-COMPARISON.md` exists to make the product decisions explicit against Codex CLI, Pi and Hermes,
and its Pi row was derived from the upstream README. A README is not enough for a **negative** claim and not
enough for a file path, so the row was re-derived from the repository itself (its file tree, its docs index,
its examples). Three cells were wrong:

- **MCP**: the row claimed "MCP, skills". The upstream tree (2,162 paths, `truncated:false`) has **no** path
  containing "mcp", the docs index (`packages/coding-agent/docs/docs.json`) has no MCP page, and the
  coding-agent README does not mention it. The cell now says skills, prompt templates and extensions, and that
  there is no MCP. (Hermes does have MCP — its docs' tools page lists `mcp-<server>` toolsets — so MCP is a
  difference against Pi, not a difference against both comparators.)
- **Worktrees**: the row claimed "worktree isolation for parallel tasks". No path in the tree contains
  "worktree", and the subagent extension's README describes process-per-subagent with isolated context
  windows and a concurrency cap (≤8 tasks, 4 concurrent). The cell now says that, and that there is no
  worktree isolation.
- **Automations**: the row cited `docs/loops.md`. That path 404s on the default branch and appears nowhere in
  the tree; the project's own README points automation and workflows at a separate repository,
  `earendil-works/pi-chat`. The cell now says so.

The sources table in that document now names what was actually read for each row (Pi: README + docs index +
tree; Hermes: README + the features/tools page) and §4 carries the re-check commands, so the next reader can
repeat the derivation instead of trusting the row:

    curl -sS "https://api.github.com/repos/earendil-works/pi/git/trees/main?recursive=1"   # 2162 paths, truncated:false
    curl -sS "https://raw.githubusercontent.com/earendil-works/pi/main/packages/coding-agent/docs/docs.json"
    curl -sS "https://api.github.com/repos/earendil-works/pi/contents/docs"                # 404: pi has no docs/ directory
    curl -sSL "https://hermes-agent.nousresearch.com/docs/user-guide/features/tools"       # Hermes MCP toolsets

Two of the corrections change the reading of the decision list in §2: item 5 (automations) no longer compares
against a Pi cron/inbox capability, and item 7 (an MCP management surface) is no longer "everyone has MCP" —
Pi has none at all. The Codex column was re-checked the same day against the installed binary
(`codex-cli 0.156.1`): every claimed verb is in its help output (`resume`/`fork`/`archive`/`delete`/
`migrate-rollouts`/`agents`, `exec` with its own `resume`/`fork`/`review`, `review`, `cloud`, `mcp
list/get/add/remove/login/logout`, `plugin`, `features`, `sandbox`, `doctor`, `debug`, `-c` overrides), and one
claim was withdrawn — "traces", because no `codex` subcommand offers one. Hermes' rows keep the strength the
sources table states (README plus the features/tools docs page, which confirms its MCP toolsets).

`docs/INSTALL.md` had the same disease one line long: it pointed at a "bundled `config.example.toml`" that
does not exist in the tree (the release workflow copies `examples/config.toml` to that name inside the
archive). The sentence now names both.

## D-108 The last two MCP options bounded nothing the tests watched (2026-09-26)

D-104's ceiling named four stdio binding options that had read sites but no behavioural test. D-106 closed
`mcp_execution` and `mcp_network` through the config edge; the two timeouts are closed here, which retires the
ceiling.

`startup_timeout_s` and `tool_timeout_s` are read once (`load_service` → `connect_stdio_in` → `startup_ms` /
`tool_ms`) and used in exactly two places (`call("initialize", …, startup_ms)` and `call("tools/call", …,
tool_ms)`), so "the key is honoured" was again a claim about code. Both tests put a *slow server* on the other
side and let the fixture's own delay be the discriminator, which is what keeps them honest without a wall-clock
assertion (the startup bound is 5 s, not 1 s: see D-115 for the fixture race that made 1 s too tight):

- `startup_timeout_s_bounds_a_silent_handshake`: the server answers `initialize` only after 30 s. With
  `startup_timeout_s = 5` the boot must fail (`MCP host initialization failed: MCP initialize timed out`); the
  60 s default would have waited the server out and booted, so the assertion is about the configured bound and
  not about "it failed eventually". The test also holds the code's own comment ("a failed handshake must
  kill+wait the server") to account: the pid the fixture wrote is gone from `/proc` afterwards.
- `tool_timeout_s_bounds_a_slow_call`: the server answers `tools/call` only after 30 s with `late answer`. With
  `tool_timeout_s = 1` the receipt the model sees carries the timeout and *not* the late answer; the 120 s
  default would have returned `late answer` after 30 s.

Both are host mode, so no bwrap: CI runs them too.

Pre-fix controls, both refuting: hardcoding `startup_timeout_s` to its 60 s default in `load_service` makes the
first test fail at `a handshake the server cannot answer inside the bound must fail boot` — the boot *succeeded*
after 30 s (30.3 s), which is exactly the state the test exists to prevent; hardcoding `tool_timeout_s` to 120 s
makes the second fail with `event goal_completed did not arrive within 15000ms`, because the call waited the
sleep out instead of reporting.

    cargo test --offline --manifest-path engine/Cargo.toml --test v2_mcp startup_timeout_s_bounds_a_silent_handshake
    cargo test --offline --manifest-path engine/Cargo.toml --test v2_mcp tool_timeout_s_bounds_a_slow_call

D-104's `tool_names` clause was too conservative as well: the negative side was already driven in-process
(`bound::tests::filtered_and_failed_services_reap_started_processes`'s "filtered" scenario — a declared list
naming only an absent tool leaves an empty surface, and the server is reaped instead of leaking), and the
config-edge cases in `v2_mcp` drive the positive side. The ceiling is therefore retired rather than shrunk.

Ceiling: the tests pin that the configured bound is *applied*, not how precisely it fires (a 1 s bound is
asserted to arrive before a 30 s answer, not within a tolerance), and the same two timeouts on the HTTP
transport still share the code path only by construction (`connect_http` takes the same two numbers).

## D-107 Nothing checked the shape of the binding decision log (2026-09-26)

While adding an entry to this file it turned out to be structurally broken, and nothing in the repository had
any reason to notice: an earlier edit had cut the tail block (D-48 … D-42) and pasted it *above* D-105, leaving
the original `## D-48` heading behind as the last line of the file with no body under it. In the committed file
that meant

    line   21: ## D-48 TUI shortcuts without function keys (2026-09-25)      <- the moved block
    line  234: ## D-105 The fourth fixture race, and the shape they all share (2026-09-26)   <- the newest entry
    line 2486: ## D-48 TUI shortcuts without function keys (2026-09-25)      <- heading only, end of file

Two headings for one entry, the newest entry 213 lines below an older block, and D-54 sitting between D-61 and
D-60 — in the file that says which rules are in force and which deviations the user confirmed. Every gate was
green, because no gate reads this file.

The file is now D-105 … D-42, strictly descending, one heading per entry; comparing the sorted lines before and
after shows exactly one removed line, the duplicate heading (the body was never lost — the whole block had
moved, heading included). The shape also has a detector, so the next edit that breaks it fails a gate instead of
being found by eye:

    python3 review/decisions_log.py    # -> 66 entries, all unique, newest-first, each with a body
    make hygiene                       # ... which now runs it inside `make check`

`review/decisions_log.py` states its four rules (a heading matches `## D-<n> <title> (<YYYY-MM-DD>)`; a number
appears once; headings are strictly descending; a heading is followed by a blank line and at least five body
lines), and it takes an optional path, which is how the pre-fix control is run: against `ead5704:docs/DECISIONS.md`
it reports exactly the four real problems (`D-48` twice, `D-42` above `D-105`, `D-54` above `D-60`, `D-48` with
0 body lines). Each rule was also exercised against a synthetic mutation — duplicate heading, dangling heading,
a block buried above the newest entry, a heading without a date, too little body — and every one produced a
complaint naming the line.

It belongs in `make hygiene` rather than in the manual-audit group with `dead_code.py`/`config_keys.py` for the
same reason the D-105 recipe belongs in the tests: the rule is mechanical and its failure is silent. Audits that
need judgment (`is this item *true*?`) stay manual, and so does the truth of an entry.

Ceiling: these are shape rules, so an entry deleted whole leaves no trace (nothing here can tell a missing
decision from one that was never written), and `make hygiene` now needs `python3` — already required by
`make pty` and by the `review/*.py` scripts.

## D-106 `mcp_execution` reached the sandbox only by construction (2026-09-26)

D-104 recorded a ceiling: the stdio binding options `mcp_execution`, `mcp_network`, `startup_timeout_s` and
`tool_timeout_s` had read sites but no behavioural test, so "this key is read" was a claim about a line of code
rather than about a running server. The first of the four is now observed through the same edge a user
configures it on.

The gap had two sides. Every other case in `engine/tests/v2_mcp.rs` passes `"host"` explicitly — `echo_catalog`
says so on purpose — and the live `review/dogfood/mcp.py` and `mcp_http.py` do too, so the *config edge* was
untested: `load_service` (`engine/src/bound.rs`) is the single line that turns `binding.mcp_execution` (default
`"workspace"`) into the bubblewrap argv, and only the direct `McpClient::connect_stdio_in` unit test in
`engine/src/mcp.rs` exercised `"workspace"` at all.

Two tests close it with the control shape this session keeps using — one server, one config key, and the host
filesystem as the witness:

- `mcp_workspace_execution_is_sandboxed`: `mcp_execution = "workspace"`; the server writes `inside.txt` in its
  cwd (the member workspace, so it must succeed) and a path under the state root outside the workspace (so it
  must fail). Asserted: the receipt carries `inside=ok` and not `outside=ok`, the host file does not exist
  afterwards, and `ws/inside.txt` is `in` — the positive half matters, or a server that cannot write anything
  would "pass" the sandbox claim.
- `mcp_host_execution_is_not_sandboxed` (the control): the same server with `mcp_execution = "host"` writes that
  host path, so the negative result above is about the sandbox and not about a server that cannot write at all.

Pre-fix control: with `load_service` hardcoded to `"host"` the sandbox test fails at `a host path outside the
workspace is unreachable: … "inside=ok outside=ok"`.

The same edge carries the network switch, and it has the same shape — `mcp_network` is off unless the user asks
for it, which is what a sandbox is for. `mcp_workspace_network_follows_the_config_key` runs one server that
reports whether it could connect to a host loopback listener in three configurations: `workspace` with no
`mcp_network` key (must observe `network=no`), `workspace` with `mcp_network = true` (must observe
`network=yes`) and `host` (the control that the measurement works). Pre-fix controls: hardcoding `load_service`
to pass `true` makes the default case fail (`the server observed … "network=yes", expected network=no`) and
hardcoding `false` makes the requested case fail (`… "network=no", expected network=yes`).

The same test also covers the branch CI takes, because GitHub runners have no bubblewrap (their kernel forbids
unprivileged user namespaces) and a test that only skipped there would leave "workspace mode never silently
degrades to the host" unchecked on the machine that runs every push. The binding is `required`, so without
bwrap the instance must not boot at all: the test runs with a `PATH` that has no `bwrap` and asserts the boot
error names the isolation (`IsolationUnavailable … MCP workspace execution requires bwrap`) and that the host
file still does not exist. Pre-fix control for that half: with the availability guard in `engine/src/mcp.rs`
removed, the same run fails with `cannot start MCP server "/usr/bin/python3": No such file or directory` — it
did not refuse honestly, it tried to run.

    cargo test --offline --manifest-path engine/Cargo.toml --test v2_mcp mcp_workspace_execution_is_sandboxed
    cargo test --offline --manifest-path engine/Cargo.toml --test v2_mcp mcp_host_execution_is_not_sandboxed
    cargo test --offline --manifest-path engine/Cargo.toml --test v2_mcp mcp_workspace_network_follows_the_config_key
    env PATH=/nonexistent <the v2_mcp test binary> mcp_workspace_execution_is_sandboxed   # the no-bwrap branch

Ceiling: D-104's ceiling shrinks by two of four — `startup_timeout_s` and `tool_timeout_s` are still read-only
claims — and `tool_names` filtering is still exercised only by the HTTP probe's single-tool binding. Recorded
rather than implied.

## D-105 The fourth fixture race, and the shape they all share (2026-09-26)

`make check` failed in `v2_driver::notify_hooks_receive_tool_call_and_run_completed`, and the failing
assertion printed the log it had read:

    the tool arguments travel on stdin: run_completed
    {"event":"run_completed",…}run_completed
    tool_call

The fixture's hook wrote the event **name** and then its payload as two separate writes, and the test's wait
condition looked for the event *names* only — so under load it could read the file between the two writes:
`tool_call` was on disk, its arguments were not, and the assertion that needs the arguments failed. Reproduced
under 16-way load: one failure in four attempts.

The fix is the same recipe as D-83/D-94/D-103, applied to a *line* instead of an event: the hook now writes one
line per event (`payload=$(cat); printf '%s %s\n' "$1" "$payload"`), and the wait condition waits for exactly
what the assertions need (`tool_call `, `echo notified`, `run_completed`). After the fix: eight consecutive
full-binary runs under the same 16-way load, all green, and `make check` green twice.

Four of these in one session is a pattern, not bad luck, and all four are the same mistake in different clothes:
**a fixture waited for something adjacent to the effect it asserts about.**

| Entry | What it waited for | What it should have waited for |
|---|---|---|
| D-83 | a fixed duration | the command having started |
| D-94 | a fixed sleep after the model's answer | the receipt the assertion reads |
| D-103 | the dispatch event (committed *before* the call is sent) | the call having reached the server |
| D-105 | the event *name* in the log | the whole line, arguments included |

Ceiling: none of these was a product defect, and all four would have been invisible on an idle machine. The
cheap mechanical guard is the one this session kept applying by hand: for every `assert!` in a fixture, find the
observation it depends on and make the wait observe *that*. A linter for it does not exist here.

## D-104 A driver that cannot boot took the coordinator down silently (2026-09-26)

The `mcp_transport = "http"` half of the MCP surface had **no test at all** — the offline suite drives stdio,
the live `mcp.py` drives stdio — so `review/dogfood/mcp_http.py` stands up a minimal streamable-HTTP MCP server
on loopback and drives it with a real model. The happy path came out as designed: with
`bearer_token_env_var = "PROBE_MCP_TOKEN"` set, the model was offered only the declared tool, its call came
back as `probe-pong-ping-1` in the conversation, and the server's log shows **all four** POSTs carrying
`Authorization: Bearer <token>` (the handshake, `tools/list`, `tools/call`, and the initialized notification).

The second scenario — the same binding with that variable **unset** — found a defect. A *required* service must
fail loudly (the offline test `required_mcp_service_failure_fails_driver_boot` pins that at the driver level),
but through the daemon the failure vanished: the supervisor's discovery loop spawned each ACTIVE instance's
driver with `?`, so the error ended the coordinator task, and the session stayed up and **silent** —

    exec report:  {'end': 'timeout', 'failure': None}   after 60 s, no event, no log line, no model request
    daemon.log:   the startup banner only

while nothing drove any instance. The user's session was wedged with a reason that existed only inside a dead
task.

The fix keeps the coordinator alive and uses the runtime's only self-action (§5.4): the instance whose driver
cannot boot is **parked with the runtime's own words**, and the discovery pass skips it afterwards (no retry
storm). Measured after the fix, the same probe:

    exec: exit 2 — "the leader instance i-leader is PARKED; new input would not run. Resume it
          (`teamagents instances resume --id i-leader`, …)"
    park reason: required tool service "probe" is unavailable: binding "probe": bearer token env var
          PROBE_MCP_TOKEN is not set

so D-82's refusal names the lever, the reason names the missing variable, `instances` shows the state, and
fixing the environment plus `instances resume` is a real recovery path.

Evidence: `v2_supervisor::a_driver_that_cannot_boot_parks_the_instance_with_the_reason` (the instance is parked
with the service and variable named, exactly one lifecycle event, and **no** `request_began`; pre-fix control —
with the old `?` restored the event never arrives and the test fails with
`event instance_lifecycle did not arrive within 10000ms`), and `python3 review/dogfood/mcp_http.py` (two
scenarios, live). `docs/USER-GUIDE.md` §5 states the behaviour.

Ceiling: the mechanism covers *any* driver-boot failure (that is the point — one place), but the probe
exercises the MCP case. The ceiling it recorded — four stdio binding options with read sites but no behavioural
test, and `tool_names` filtering only through this probe — is retired: D-106 (the execution mode and the network
switch) and D-108 (the two timeouts) took all four through the config edge, and `tool_names`' negative side was
already an in-process test (`bound::tests::filtered_and_failed_services_reap_started_processes`).

## D-103 A third fixture waited for the wrong thing: the MCP crash window (2026-09-26)

`make check` failed in `v2_mcp::recovered_mcp_dispatch_is_not_replayed` with

    panicked at tests/v2_mcp.rs:298: called `Result::unwrap()` on an `Err` value:
    Os { code: 2, kind: NotFound, message: "No such file or directory" }

which is the read of the marker file the fixture's Python MCP server writes on every `tools/call`. Reproduced
under load (eight copies of the single test in parallel while the binary ran): one failure in two attempts.

The cause is the same shape as D-83 and D-94, one step further out: the fixture waited for the
`operation_dispatched` **event** — committed *before* the call is sent — and then crashed the driver at once.
Under load the server had not received `tools/call` yet (its `initialize`/`tools/list` handshake plus the
driver's send take time), so the marker did not exist, and the test's actual claim — *a call that really
started is not repeated after a crash* — was not under test at all: the crash could have landed before the
effect.

The fix is the recipe the other two now use: wait for the **effect**, not for a duration or for the event that
precedes it. The fixture waits (bounded at 10 s) until the marker exists and asserts it did — which both makes
the window deterministic and strengthens it, because the assertion that follows ("exactly one `called` line")
now genuinely means "it was started once and never replayed". The failing read also turns into a named
assertion (`the fixture's MCP server never recorded the call, so nothing was in flight`) instead of a bare
`unwrap`.

Evidence: before — under eight-way load, one failure in two attempts, always at the marker read; after — three
consecutive full-binary runs under the same load, all green, and `make check` green twice.

Ceiling: the 10 s bound is still a bound (a machine slower than that fails the *named* assertion, which is
what it is for). The pattern is worth stating plainly, because it has now cost three diagnoses in this
session: **a fixture must wait for the effect it asserts about** — the deadline fixture slept while the
command had not started (D-83), the supervisor fixture read a receipt the runtime had not written yet (D-94),
and this one crashed a window the effect had not entered.

## D-102 `instruction_files` promised a prompt nothing reads (2026-09-26)

D-75's rule is that a config key this build does not serve is *made to work, refused with a pointer, or
reported as not in effect*. The sweep behind D-75 covered `[permissions] mode`, `[retention]` and
`models.*.codex_profile`; `instruction_files` looked served, because `doctor` prints

    [ok  ] instruction files        1 file(s) reach every member's prompt

It is not served. Measured 2026-09-26 (`python3 review/dogfood/instructions.py`): with a file holding
`CANARY_INSTRUCTION_9F2A` configured, the canary is absent from the leader's prompt — and that prompt is
inspectable, because it is the profile's `instructions`, which is exactly what `core/src/kernel/instance.rs`
pushes as the system message. The only readers of the key are the loader (parsing), the validator (the path
must exist) and that doctor row; the skills registry is built from `skills_paths` alone.

**What it gets, and why.** `doctor` now reports `1 declared, not applied: this release does not read
instruction files into a prompt (a member's instructions come from its own profile)`, and still names a path
that does not resolve. The feature itself is *not* implemented here, and the reason is the repository's own
rule rather than the size of the change: the design baseline does not mention the key at all, so wiring it —
appending the files' text to every member's prompt at prompt-build time, one place, user-config-only — is new
design and needs the user's word. It is recorded as a known gap in `docs/ACCEPTANCE.md` with that sketch, the
same way `[retention]` is.

The probe pins the *current* truth so the promise cannot come back silently, and it is written to flip: it
requires the file to really hold the canary, the prompt to exist and be inspectable, the canary to be **absent**
from it, and `doctor` to say "not applied". When the feature lands, the third check becomes "the canary is in
the prompt" and the probe is its acceptance test.

The class keeps recurring, so it also has a detector now: `review/config_keys.py` reads the config structs out
of `core/src/models.rs` and reports every field whose mentions outside the loader, the validator, the doctor
surface and the argv parser are none — the config-side sibling of `review/dead_code.py`. First run over this
tree: 45 fields, **0 unserved**, four on its allowlist with their reason (`codex_profile` refused, D-75;
`archived_days`/`history_days` reported as not applied, D-75; `instruction_files`, this entry). Its limits are
stated in its docstring — the check is name-based, and it cannot tell "read for a report" from "read to act",
which is what the allowlist is for.

Evidence: `cli::doctor_reports_the_skills_registry_and_missing_configured_paths` (the `[WARN]`/`not applied`
assertions; with the old wording the test cannot pass, which is its counterfactual), the doctor output above,
`python3 review/config_keys.py` (the negative result for the rest of the config surface), and
`python3 review/dogfood/instructions.py` (two runs). `docs/USER-GUIDE.md` §5 no longer implies the files
reach a prompt and points at what does work today (a member's instructions are its profile; the Leader passes
rules in `spawn`/`delegate` text).

Ceiling: the probe measures the *prompt the leader was given*; a member's prompt follows the same profile path
(the spawn path copies the parent profile and overrides `instructions` from the tool call), so the finding
generalises by construction rather than by a second live run.

## D-101 The two acceptance mechanisms in one run, and which one decides what (2026-09-26)

The product has two acceptance mechanisms — `[[checks]]` from the user config, which gate the **goal** at the
completion boundary (§8/A16), and `exec --check`, the **client's** own command, which decides the **exit
code** of a finished turn (D-49) — and nothing documented or drove what happens when both are configured at
once. `review/dogfood/two_gates.py` does, with two scenarios that separate the roles:

| Scenario | Goal | Exit code | Client verdicts |
|---|---|---|---|
| the runtime check can never pass; the client's check passes | `BLOCKED` (2 repair rounds reported, the ledger names `runtime-gate`) | `1` | `test -f hello.txt` → `ok: true` |
| the runtime check passes; the client's check fails | `SUCCEEDED` | `1` | `test -f missing.txt` → `ok: false`, exit `1` |

So the division of labour is exactly as designed and now measured from both sides: the runtime's gate decides
whether the goal may be reported done, the client's commands decide the exit code — and neither replaces the
other (a passing client check does not rescue a blocked goal; a settled goal does not rescue a failing client
check). Measured 2026-09-26, ~13 s and ~5 s per scenario, each on its own fresh state root (runtime checks
belong to the goal).

One event-level detail this pinned, because it is what an integration would parse: in the first scenario the
goal ended `BLOCKED` **because the model itself reported `blocked`** after the repair turn showed it the
failing check (its completion summary says the deliverable exists but the gate cannot pass), not because the
repair budget ran out — the runtime accepts either honest route. The check's name therefore lives in the check
ledger (`completion_repair.failures[].check_id`), which is where the probe reads it, and not necessarily in
the completion event's own fields.

Evidence: `python3 review/dogfood/two_gates.py` (two runs), and the probes it complements — `checks.py` (A16,
a check that cannot pass), `stale_check.py` (A17, a verified input that changed) and `exec_check.py` (the
client's `--check` in three scenarios).

Ceiling: two scenarios; the repair-round budget (`max_check_rounds`), per-check `timeout`/`network` options and
the interaction with `[[checks]]` *inputs* stay with the offline tests. `docs/USER-GUIDE.md` now states the
division in one sentence next to `--check`.

## D-100 A reset mid-run, and the one place that decides a run's fate (2026-09-26)

The fourth member of the family D-97/D-98 opened: `reset_instance` (a user command; there is no CLI verb, so
the probe speaks the documented protocol) closes the epoch's execution and moves the instance to a new epoch.
A run waiting on a turn in the old epoch therefore has nothing that will ever answer it — and the client
waited out its deadline and reported `end: "timeout"`, measured with a real model:

    reset reply:  {'epoch': 1, 'closed': {'requests_closed': 1, …}}
    exec report:  {'end': 'timeout', 'goal_status': None, 'failure': None}       (40 s deadline)

Rather than add a fourth branch, the client now has **one** place that decides this: `run_fate(event,
instance) -> Option<String>` maps the events that end a run while its turn can never finish — a refused
request (budget ceiling, goal deadline), a retired instance, a reset — to the runtime's own words, and the
loop has a single call site. The next such fact is one arm in that match instead of a new branch, which is
what the last three defects all would have been.

The reset case itself, measured live (`review/dogfood/lifecycle_run.py --lever reset`): the run ends **0.2 s**
after the reset with exit `1` and `failure: "instance i-leader was reset while this run was in flight (epoch
0 → 1); its turn is gone, so nothing will answer this input"`.

One ordering had to change with it: a *queued* input that a reset seals is named by the runtime in
`envelopes_sealed`, and D-72 makes that the input's own outcome (`undelivered`). Since a reset now also sets
"this run failed", the terminal match checks `undelivered` **first** — a fact the runtime stated about *this
input's envelope* is more precise than a generic failure, and both are true after a reset. The existing test
for that path (`a_queued_input_a_reset_sealed_is_reported_undelivered`) is the guard.

Evidence: `v2_daemon::a_reset_mid_run_ends_the_headless_run_with_the_reason` (pre-fix control: `Timeout`),
`v2::exec::tests::the_facts_that_end_a_run_are_named_in_the_runtimes_words` (all four fates plus the two
non-fates: `request_failed` and a *pause*), the D-72 regression test above, and the live probe in all three
lever modes (terminate / reset / pause, measured 2026-09-26).

Ceiling: `run_fate` covers the events the control plane commits today. A future terminal-for-this-run fact
still has to be added there — the difference is that it is now one arm next to the others rather than a new
code path, and the unit test lists them together so a missing one is visible in one place.

## D-99 The TUI kept saying "disconnected" while it was connected again (2026-09-26)

A28's client half had the pieces — `mark_disconnected`/`mark_connected`, a fresh checkpoint and a history
reload on reconnect — and no live run of the transition. `review/dogfood/tui_reconnect.py` does it: attach the
TUI, kill the daemon, watch the client notice, start a new daemon on the same state root.

The first run found a stale indicator:

    status line: … usage 0 · disconnected, reconnec      (90.3 s after the new daemon was up)
    the panel shows an instance created after the restart: the client is live

So the client *had* reconnected (it fetched a checkpoint and rendered an instance created after the restart)
while the status line still claimed it was disconnected. The cause is a redraw, not a connection: the
150 ms event branch calls `app.mark_connected()` and then only sets `dirty` when events arrived
(`if !events.is_empty()`), and a reconnect that delivers no events is exactly the quiet case. The flag was
cleared; the frame was never rebuilt, so the old text stayed on screen until something else forced a draw.

The fix is one rule in the loop: the connection flag is compared every iteration and a change rebuilds the
frame, whichever path cleared it. Measured after the fix (three runs): the status line clears **0.6 s** after
the new daemon answers.

Two things are worth keeping from this one:

- **a stale indicator is worse than a missing one.** "disconnected, reconnecting…" is the only signal that
  tells a user the panel data may be old; once it can be wrong in that direction, it stops being a signal.
- **the probe's first version could not have found it.** It asserted that the *instance row* was back — but
  the row was still painted from before the kill, so it passed on stale text (D-84's class again). The
  shipped assertion creates a **second** instance *after* the restart and waits for *that* row: only a live
  client can render it.

Evidence: `python3 review/dogfood/tui_reconnect.py` (three runs, model-free, ~25 s: attach → panel live →
SIGKILL the daemon → the client says disconnected → a key meanwhile reports `command failed` in the
conversation → a new daemon → the status line clears in 0.6 s → an instance created after the restart appears
in the panel) and the new client-level test `tui/tests/reconnect.rs` (a real socket that dies and rebinds:
`call` fails and marks the client disconnected, then succeeds and clears the flag — written while isolating
the client logic from the loop, and kept because the unit tests only ever scripted a healthy connection).

Ceiling: the redraw rule itself has no unit test — the loop needs a terminal, so the probe is its evidence.
The probe drives a *killed* daemon and a fresh one on the same state root; it does not exercise a daemon that
is merely slow (the 2 s I/O timeout and the retry cadence stay with the scripted client tests).

## D-98 A run whose instance is stopped mid-flight (2026-09-26)

The user's two lifecycle levers behave differently while a run is waiting, and the headless client treated
both as "still running".

**Terminated.** `instances terminate --id … --yes` closes the instance's open execution: nothing will ever
answer that run. Measured 2026-09-26 with a real model, `exec` waiting on a long turn and the leader retired
through the CLI:

| | before | after |
|---|---|---|
| report | `end: "timeout"`, `failure: null` | `end: "failed"`, `failure: "instance i-leader is terminated; this run cannot finish (termination is final — start a fresh state root for new work)"` |
| exit code | `124` | `1` |
| wall clock after the lever | 118.8 s (the caller's deadline) | 0.2 s |

The client now watches `instance_lifecycle` for the instance it submitted to. The exit code differs on purpose
from D-82's at-submit case (`2`, "nothing was submitted"): here a turn really ran and is unfinished, which is
what `1` means.

**Paused.** My first fix treated `PAUSED` the same way — and the pre-fix control of the scripted test refuted
it (`left: Reply, right: Failed`): a pause stops the instance at a *boundary*, and the attempt in flight can
still land and close the turn, so a run must keep following its own turn. The shipped rule is therefore:

- `terminate` ends the run at once with the truth;
- `pause` does **not**: the run keeps waiting, and a resumed instance continues the work — measured live,
  pause mid-run → resume five seconds later → the *same* run ends `exit 0 / end=completed / goal
  SUCCEEDED`, with the file the turn was asked for on disk (`review/dogfood/lifecycle_run.py --lever pause`);
- if the caller's deadline wins instead, the report says **what the instance was doing** rather than the
  wrong "still running": the report now carries `instance_lifecycle`, and the one-line verdict distinguishes
  `ACTIVE` / a stopped lifecycle (name the `instances resume --id …` lever) / `TERMINATED` (termination is
  final). `timeout_line` is a pure function with its own unit test.

Evidence: `v2_daemon::terminating_the_leader_mid_run_ends_the_headless_run_at_once` (pre-fix control:
`Timeout`), `v2_daemon::pausing_the_leader_mid_run_lets_the_turn_finish` (the pause arrives while the request
is in flight; the reply still closes the turn, the run reports `Reply` and the instance really is `PAUSED` —
this test is what stopped the wrong fix), `v2::exec::tests::a_deadline_says_what_the_instance_was_doing`, and
the live probe `review/dogfood/lifecycle_run.py` in both modes (2026-09-26, three runs).

Ceiling: the client watches the lifecycle of the instance it submitted to; another instance being paused or
retired is not this run's subject and stays ignored. A pause whose resume happens after the caller's deadline
still ends as `timeout` — truthfully the caller stopped waiting — and the message now names the lifecycle.

## D-97 A refused request is not a slow one: `exec` waited out its deadline (2026-09-26)

The runtime refuses a request *before it begins* in two cases, both as committed outcomes with an auditable
event: the goal's budget ceiling cannot pay for it (`budget_refused`, A18) and the goal's deadline has passed
(`goal_deadline_refused`, A35). The headless client knew only about `request_failed`, so a run whose request
was refused waited for its own deadline and then reported `end: "timeout"` — the exit code that means "the
instance is still running" — while the session had already parked the leader with the real reason.

Measured 2026-09-26, `[limits] max_total_tokens = 1000` and a real model (the ceiling is below one request's
estimate, so no model is ever called):

| | before | after |
|---|---|---|
| report | `end: "timeout"`, `failure: null` | `end: "failed"`, `failure: "goal goal-s-main budget exceeded: known 0 + reserved 0 + est 2235 > max 1000"` |
| exit code | `124` | `1` |
| wall clock | 60 s (the caller's deadline) | 1 s |

The session was never wrong; the client was. The fix makes both refusal events part of the run's own outcome:
the client composes the runtime's sentence from the event payload (`startup_refusal`) and reports the run as
failed, exactly as it already did for a permanent model failure (D-49's "a failed turn ends the headless run
instead of timing out"). `124` keeps its documented meaning, and a refusal now tells the user what to change —
the ceiling or the deadline — in the first second instead of the last.

Evidence: `v2_daemon::a_budget_refusal_ends_the_headless_run_instead_of_timing_out` (a 1-token ceiling; the run
must end `Failed`, exit 1, with `budget exceeded` and `max 1` in `failure`, well inside a 30 s deadline). The
pre-fix control: with the new branch disabled the same test reports `end: "timeout"` — the client really did
wait out the deadline. Live: the run above against the real binary, both before and after.

**The deadline probe then caught a gap in this very fix.** D-96 taught `exec` that a turn which never finished
has nothing to verify, but it keyed that on `End::Timeout` — and a refusal ends as `End::Failed`, which *is* in
the "checks run" set. So `--check` still ran for a request that never began: measured in
`review/dogfood/deadline.py`'s first run, the refused turn executed `touch …/check-ran`, reported
`verification: [{… "ok": true …}]` and wrote a ledger. A run whose request was refused now skips the checks
too (the refusal is remembered separately from `turn_failure`), and the same daemon test asserts it: no marker
file, an empty verdict list, a null `verification_path`.

Ceiling: the two refusal events are the only "request never began" outcomes the control plane commits; a
refusal that arrives *after* an attempt started is a different case and still ends as its own failure class
and does run the checks. `docs/USER-GUIDE.md` states the rule for the user.

## D-96 `--timeout` did not bound a run that had checks (2026-09-26)

D-49's contract says the user's acceptance commands "run after the turn ends", and the code's own comment
enumerated the cases where nothing runs: an approval stop (the turn is not finished) and an input that never
landed (there is no turn to accept). A caller's deadline passing was missing from that list, so `exec
--timeout 3 --check "…"` on a turn that never finished ran the checks *after* the deadline had already
passed.

Measured 2026-09-26 (the same command, before the fix): `--timeout 3` with `--check "sleep 5; test -f
nothing.txt"` exited 124 after **7 s** of wall clock, and the check — which inherits `timeout_s`, 3 s here —
was recorded as failed with `command timed out after 3s`, written to `<state root>/verification.json` and
reported as `verification_path`. Two things are wrong with that: a run can outlive the deadline its caller
set (with a real `cargo test` check the overshoot is the check's whole duration), and the ledger then holds
verdicts for a turn that never finished, which is exactly what the "not finished → nothing to verify" rule
exists to prevent.

The fix adds `End::Timeout` to that rule, so a run past its deadline verifies nothing: `verification: []`,
`verification_path: null`, no ledger, no check process, and the exit code stays `124`. Nothing else changes —
checks still run after a *finished* turn, including a failed or blocked one, and still return exit `1` when
they fail.

Evidence: `v2_daemon::a_run_that_times_out_verifies_nothing` (a scripted provider that answers 1.5 s after a
1 s deadline; the run must exit 124 with an empty verdict list, a null path and no marker file from the
check, and must finish well inside the bound). The pre-fix control is in the same test's history: with
`End::Timeout` removed the assertion fails with `verification: [{"command":"touch …/check-ran","ok":true,…}]`
and a non-null `verification_path` — the check really ran and really wrote a ledger. `docs/USER-GUIDE.md` sets
the same expectation in the user's words ("checks are skipped when the turn is not finished — an approval, or
your own `--timeout` deadline").

Ceiling: the check's own timeout still inherits `--timeout` (`ExecOptions.timeout_s`); a CI job that wants a
long check under a short turn deadline has no separate knob today, and `[[checks]]` in the config (D-50) is
the surface that does have one. Recorded rather than invented here.

## D-95 The instances panel's keys, against a real daemon (2026-09-26)

The panels had exactly one kind of coverage: the PTY smoke drives them against a *scripted* daemon and asserts
the frames the keys produce. That is a real check of the key map, but it cannot answer the question a user
asks of a panel — does the key act on the row I selected, does the daemon change state, and does the panel
show me the new state?

`review/dogfood/tui_panels.py` answers it with a real daemon and the real TUI, and without a model: the
session starts, the probe creates one extra instance through the documented protocol (no user surface creates
one without a Leader, and the panel's subject is the row, not the model), attaches the TUI to a PTY, and then
presses what a user presses.

Measured 2026-09-26 (three runs, ~9 s each, no credential needed):

| Step | Result |
|---|---|
| `Ctrl+N` | the instances view opens and lists both rows (`i-leader · ACTIVE`, `i-worker · ACTIVE`) |
| `Down` | the selection marker moves to the worker's row (`▶  i-worker`) — the row the keys below act on is the one the user picked |
| `p` | `teamagents instances --json` reports `PAUSED` **and** the panel repaints `i-worker · PAUSED` (the refresh comes from the daemon's own `instance_lifecycle` event) |
| `r` | `ACTIVE` again, on both sides |
| `t` | the footer asks `terminate this instance? y confirm / n cancel` and the instance **stays** `ACTIVE` |
| `n` | the prompt disappears and the instance is still `ACTIVE` |
| `t`, `y` | the instance is `TERMINATED` and the panel shows `TERMINATED` |

The probe also drives the *tasks* view, with a task it delegates itself as the user (`delegate_task` accepts
`Identity::User` when the goal is named): `Ctrl+N` opens the view with the new row selected (`▶ t-panel`),
the row is `RUNNING` (the assignee picked it up), `c` cancels it in `teamagents tasks --json` **and** the
panel repaints `t-panel · CANCELLED`. `Esc` returns to the conversation and `Ctrl+N` re-enters the instances
view, where the selection starts on the conversation target again — the probe moves it back to the worker
before the termination stage, so the two stages cannot silently act on each other's row.

The two assertions that matter most are the last four rows: termination is irreversible for the session, so
the panel's `t` must be a question rather than an action, and a cancelled question must leave the row exactly
as it was. Both are observable only against a daemon that keeps the state (D-82).

Evidence: `python3 review/dogfood/tui_panels.py` (three runs), plus the layers it does not repeat — the PTY
smoke's frames against the scripted daemon (D-68's key map), `v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance`
(the same transitions through the CLI) and `tui::the_instances_hint_stops_offering_lifecycle_keys_for_a_terminated_member`
(the hint after retirement).

Ceiling: the topology view still rests on the scripted smoke (it renders edges, it has no action of its own),
and the *delegation-level* consequence of a cancelled task — the delegator waking — is `cancel.py`'s subject
(D-88) and the D-68 daemon test's. The probe does not exercise a selection beyond one `Down`; a session with
several members would need the same key sequence per row.

## D-94 A gate flake with a precise cause: a receipt read once after a fixed sleep (2026-09-26)

`make check` failed once in `v2_supervisor::a_settled_goal_leaves_a_later_delegation_without_an_active_goal`
— the test that pins the known gap "a settled goal leaves no surface to open a new one". The panic said
exactly where:

    panicked at tests/v2_supervisor.rs:943:
    no delegation refusal in the receipts: ["…[finish accepted: goal goal-s-test closed as SUCCEEDED]"]

The fixture drove the second instruction, waited for the leader's second *model answer* to appear, then slept
a fixed 300 ms and read the conversation once. The receipt for the delegation the model attempted lands after
that answer, so under a loaded suite the read saw only the first turn's `finish` receipt. Reproduced on
demand by running the whole `v2_supervisor` binary with sixteen copies of the single test in parallel: one
failure in three or four attempts, always at the same line.

The fix is the same class as D-83's: wait for the *condition* instead of for a duration. The test now polls
its own read of the leader's tool results (every 50 ms, 20 s bound — the same bound its other waits use) until
the refusal is there, and only then asserts the goal set and the refusal's wording. After the change: six
consecutive full-binary runs under the same sixteen-way load, all green, and `make check` green twice.

Ceiling: the flake was load-dependent and is now rare rather than impossible — a 20 s bound is still a
bound; the real fix for a suite like this is a deterministic "wait for the runtime to be idle" helper, which
this repository does not have (each test drives its own supervisor and observes events instead). Recorded
here so the next reader of that fixture knows why it polls.

## D-93 `exec --check` driven the way a CI job uses it (2026-09-26)

`teamagents exec --check COMMAND` is the contract a CI job depends on: the turn runs, then the user's own
acceptance commands run in the client's workspace, in order, and decide the exit code. Its evidence was unit
tests plus a real daemon with a *scripted* provider, and the rule that a command printing `(exit 0)` cannot
fake a pass was unit-only. `review/dogfood/exec_check.py` runs three scenarios against one real session with
a real model (`--full-auto`, so the checks run on the host):

| Scenario | Result (2026-09-26, DeepSeek Flash) |
|---|---|
| a passing check after a turn that really wrote `hello.txt` | exit 0, `end=completed`, goal `SUCCEEDED`, ledger `ok: true / exit_code: 0` |
| a failing check, with a second check after it | exit 1, and the ledger holds **one** row — the list stopped at the first failure — while `hello.txt` from the first turn is still there |
| a command that prints `(exit 0)` and exits 7 | exit 1, ledger `exit_code: 7 / ok: false`, and the command's own text stays in `output` |
| a second session **without** `--full-auto`, where the turn parks on an approval | exit 3, `verification: []`, `verification_path: null`, and the ledger the earlier passing run in that same session wrote is **untouched** (an approval stop verifies nothing, §8) |

Two things this pins down that the unit tests could not: the checks run *after* a turn that is itself a plain
reply (scenarios 2 and 3 report `end=reply`, because the goal settled in scenario 1), and the artifact — the
file the first turn wrote — survives, which is what makes "the checks gate the exit code, not the work" a
statement about the product rather than about the checker.

Evidence: `python3 review/dogfood/exec_check.py` (two runs), plus the offline layers it does not repeat —
`v2::exec::tests::acceptance_commands_run_in_order_and_stop_at_the_first_failure`,
`the_check_verdict_reads_the_wrapper_marker`, `exit_codes_follow_the_documented_contract`,
`v2_daemon::headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code`,
`a_failing_acceptance_command_fails_the_run`, and `cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check`.

That fourth scenario also settles what the ledger is: `write_verification` returns `None` for an empty
verdict list, so a run without verdicts writes **no** file and reports `verification_path: null` — the earlier
run's ledger stays exactly as it was. The reliable signal for a CI job is therefore the path in *its own*
report, not the presence of `<state root>/verification.json` (which may belong to an earlier run).

Ceiling: one passing check, one failing list, one forged marker and one approval stop per run; the remaining
contract points (checks in order across several *passing* commands, the timeout of a hanging check, a check
whose shell cannot start) stay with `v2::exec`'s unit tests and the daemon tests above.

## D-92 The user's policy hook, driven with a real model and real tool calls (2026-09-26)

`[hooks]` is the one surface where the *user's own programs* wrap the runtime's work: `notify` on events,
`pre_tool` in front of every native tool call. Its contract (exit 0 allows, **exit 2 denies** with the first
stderr line as the reason, anything else — other exit codes, a spawn failure, a 10 s timeout — allows, and a
hanging `notify` never blocks a turn) was covered by unit tests in `engine/src/hooks.rs`, all in-process:
generated scripts, no daemon, no model, no real tool call. A user's policy that silently denies everything, or
a "broken hook" rule that brick the agent, would have looked identical to a passing test.

`review/dogfood/hooks.py` drives it end to end. It generates two scripts in the scratch root — `notify.sh`
(appends `argv[1]` and the JSON payload it reads on stdin) and `veto.sh` (appends the payload it is asked
about, then denies the `shell` tool by exit code) — and asks a real model for two things in one turn: write
`kept.txt`, then run `echo veto-me > vetoed.txt`.

Measured 2026-09-26 (DeepSeek Flash, native window, `--full-auto`):

| Mode | Result |
|---|---|
| `veto` (exit 2) | the hook was consulted 4 times, once about `shell`; `kept.txt` holds `kept` (the *file* tool was allowed), `vetoed.txt` never exists, and the model's conversation carries `{"error":"denied by pre_tool hook: probe rule: shell commands are not allowed in this project"}` — the reason reached it verbatim. The model then verified the state and finished **`blocked`** of its own accord (step 2 impossible): exit 1, goal `BLOCKED` |
| `broken` (exit 1) | the same hook exiting 1 **allowed** the call: `vetoed.txt` holds `veto-me`, no denial appears anywhere, exit 0, goal `SUCCEEDED` |

The `notify` stream in those runs carried 8–11 events per turn — 4–6 `tool_call` (with `tool`, `arguments` and
`ok`, **including `ok: false` for the denied call**) and 4–5 `run_completed` — every payload valid JSON with
the `session_id` and the instance id. That last count also corrected a document: `run_completed` fires per
**model request** (its payload names the `request_id`), not per turn, and `docs/USER-GUIDE.md` §2.3 said only
"`run_completed`". It now says what the granularity is.

The negative control is what makes the first run mean anything: "the veto worked" is also consistent with an
implementation that denies every call, and "anything else allows" is exactly the branch a policy hook is
easiest to get wrong.

Evidence: `python3 review/dogfood/hooks.py` and `--mode broken` (2026-09-26, three runs), plus the offline
layers it does not repeat — `hooks::tests::pre_tool_policy_decides_by_exit_code` (exit 0 allows, exit 2
denies, a broken hook allows, a hanging hook is bounded and allows), `hooks_receive_the_event_name_and_json_on_stdin`,
`v2_driver::a_pre_tool_hook_vetoes_a_tool_call_and_the_turn_continues`,
`v2_driver::notify_hooks_receive_tool_call_and_run_completed`, and `cli`'s doctor reporting both configured
argv vectors (`hooks.notify` / `hooks.pre_tool`).

Ceiling: one turn per mode, so the 10 s timeout, the "a chatty hook cannot fill the pipe" case and the
"replays after crash recovery are not asked again" rule stay with the unit tests; the probe's scripts are
POSIX `sh` using `grep`, which the host provides (hooks run on the host by design, never inside an instance
sandbox).

## D-91 The running job's identity, and its one-start rule, checked against the machine (2026-09-26)

A15 and A10 both rested on offline tests (`jobs_runner`) plus code reading: the runner persists the child's
pid, its `/proc/<pid>/stat` start ticks and the machine's boot id "because pid alone never proves identity"
(§6.2), and GO is idempotent "so a duplicate GO starts exactly one command" (A10). Neither had been checked
against a real running command on a real machine. `review/dogfood/job_identity.py` does that: the Leader
hires a worker, the user grants it `shell@workspace` and sends it a command that loops forever, and while the
job is `RUNNING` the probe

- reads `<state root>/instances/<id>/jobs/<operation>/journal.json` and **re-derives** `start_ticks` (field 22
  of `/proc/<pid>/stat`) and `boot_id` (`/proc/sys/kernel/random/boot_id`) itself, so the comparison is
  between the runner's record and the machine rather than the record with itself;
- asks the runner over the socket name the *token* derives (§6.2) and requires its own view of the identity
  to agree with the journal;
- sends a **duplicate GO** over the same socket and requires the reply to carry `starts: 1` and the same pid;
- connects with a *guessed* token-derived name and requires the connection to be refused.

Measured 2026-09-26 (DeepSeek Flash, `--full-auto`, two runs): the journal's `start_ticks` matched
`/proc/<pid>/stat` exactly, the recorded boot id matched the machine, the runner agreed with its own journal
over the token socket, the duplicate GO left `starts` at 1 with the pid unchanged, and a guessed token got
`ConnectionRefusedError` (the abstract socket name is a hash of the secret, so a guess cannot even reach the
listener).

Evidence: `python3 review/dogfood/job_identity.py` (two runs), plus the offline layers it does not repeat —
`jobs_runner::duplicate_go_starts_exactly_one_command`, `cancel_before_start_persists_and_rejects_late_go`,
`cancel_running_stops_the_process_group`, and the identity re-check in `jobs::signal_group` (which refuses to
signal when `boot_id` or `start_ticks` no longer match).

Ceiling: the probe finds the job by walking the state root, so it observes identity while the command is
still running; a job that ends before the probe looks (a model that passes its own `timeout` to the shell
tool, which one run did before the instruction forbade it) is reported as such instead of being skipped. The
recycled-pid case — a *different* process holding the recorded pid — is the one the identity check exists
for, and it is not producible on demand; the offline suite covers the refusal path.

## D-90 The completion gate is not the check's exit code, measured live (2026-09-26)

A17's evidence was one driver test with a scripted provider plus a config test. The claim is subtler than
A16's ("a failing check blocks"), so it deserved a live run: *a check that passed does not count if the thing
it verified changed*, and the gate's unit is the (result, declared inputs) pair, not the exit status. The
deterministic way to make the two disagree is a check that rewrites the very file it declares as its input —
which is also the shape the offline test uses.

`review/dogfood/stale_check.py`: one `[[checks]]` entry, `id = "bound"`, `command = "printf changed > out.txt"`,
`inputs = ["out.txt"]`; the model is asked to write `out.txt` with the content `original` and to finish.

Measured 2026-09-26 (native windows, `--full-auto`):

| Provider | `exec` | wall clock | check rounds | model requests | goal | reason |
|---|---|---|---|---|---|---|
| `deepseek` | exit 1, `end=failed` | 19.7 s | 3 | 12 | `BLOCKED` | `required checks failed (bound:stale_inputs) after 3 round(s)` |
| `kimi` | exit 1, `end=failed` | 56.6 s | 3 | 9 | `BLOCKED` | same |

Both runs also show the two halves the probe insists on: the model really did the work (its write of `out.txt`
is in the conversation) and the check really ran (the file ends up holding *the check's* content, which is how
the probe knows the gate saw a different value from the one it observed), while no goal was ever reported
`SUCCEEDED` and the verdict reached the model as a tool result — it got two bounded repair rounds before the
goal was blocked with the failing id.

Evidence: `python3 review/dogfood/stale_check.py` and `--provider kimi` (2026-09-26), plus the offline layers
it does not repeat — `v2_driver::check_inputs_must_still_hold_at_completion` (the same shape with a scripted
provider), `config::user_checks_become_goal_limits` and `v2_driver::configured_checks_gate_the_goal_through_the_config_edge`.

Ceiling: the probe makes the input change *inside* the check, which is deterministic but narrower than "some
other process changed a build artifact between the check and completion" — that path is the same code
(`observe_check_inputs` per round, re-verified at completion) but is not what this run drives. The wall clock
is provider-dependent (kimi took three times as long for the same three rounds), which is why the probe
records it instead of asserting a bound.

## D-89 The dangerous call, decided in the TUI with a real model (2026-09-26)

The one place where the product stops and asks the user is a gated call, and that decision had two half
coverages: `v2_daemon::the_approvals_cli_lists_and_decides_a_parked_operation` drives the *CLI* against a
real daemon with a scripted provider, and the PTY smoke paints and clicks the TUI's approvals box against a
*scripted* daemon. Neither put the two together, so "the user reads a real call's arguments and decides in
the TUI" was unproven. `review/dogfood/approval.py` does: the daemon starts **without** `--full-auto`
(`require_shell_approval = !full_auto`, so every `shell` call parks), the real TUI attaches, the probe types
a prompt asking for `echo approved-live > proof.txt`, and then presses the TUI's own keys — `Ctrl+A` into the
box, `a` or `d` — while the pending id has to be on screen first.

Measured 2026-09-26 (DeepSeek Flash, native window, `approved_scope` so the approved command runs inside
bubblewrap):

| Decision | Result |
|---|---|
| `a` (approve) | the pending id and the exact call (`ap-…:0 · shell · echo approved-live > …/proof.txt`) were both on screen and in `teamagents approvals --json`; after the key the session recorded `APPROVED`, the file appeared with the content, the box dropped the id and the session had nothing pending |
| `d` (deny) | the session recorded `DENIED`, `proof.txt` never appeared, and the operation landed `CANCELLED` with a receipt whose `class` is `denied` — the call failed closed and the model was told (the receipt is in its context) |

Two probe lessons, the same family as D-84's "an assertion must be able to fail for the reason it names":

- **an absence check must be the absence of a proven present thing.** The first version waited for the text
  `0 pending approval`, which the UI never prints (the status line shows `· N pending approvals` only while
  something is pending, and the footer counts it). The fix is not a different string but a pair: assert the
  *decided id* is on screen, then wait for that id to leave — and assert the decided id is gone from the
  session's pending list, not that the list is empty, because a model may legitimately ask for a *second*
  decision in the same turn (observed: it did, which is what made the weaker assertion fail).
- this is the only probe of the set that runs the shell **inside bubblewrap**: every other one passes
  `--full-auto` (host shell, no approval), so the isolated path picks up a live witness here for free.

Evidence: `python3 review/dogfood/approval.py` and `--decision deny` (five runs on 2026-09-26), plus the
offline layers it does not repeat — `the_approvals_cli_lists_and_decides_a_parked_operation`,
`approval_flow_blocks_then_allows_dispatch`, `pending_approvals_expire_when_the_operation_closes`,
`shell_approval_blocks_then_allows_and_denial_cancels` (the denial path, driven: it parks, the second
approval is taken, and the denied call cancels), and the PTY smoke's frames.

Ceiling: one decision per run — the `once` binding's reuse and expiry are offline tests, not re-measured
here; the probe focuses the box with `Ctrl+A` on a single-row list, so the `↑`/`↓` selection over several
pending approvals stays with the scripted smoke.

## D-88 How a user stops a command that is already running (2026-09-26)

A13 ("cancel, timeout and completion races") had only offline evidence — `jobs_runner` and `v2_driver` with
scripted providers — and while building the live probe the question turned out to have a surprising shape:
**there are two levels, and only one of them stops work.**

- `teamagents tasks cancel --id …` is **delegation-level**. It marks the task `CANCELLED`, revokes the
  return path, wakes the delegator and queues a `task_cancelled` envelope to the assignee — which the
  assignee applies at *its* next boundary. The assignee's running command keeps running: §6.4 makes
  cancellation a "process-group stop request to a controlled job", and a job belongs to an **operation**,
  which a task cancel does not touch. Measured (2026-09-26, DeepSeek Flash, native window): the delegated
  command kept ticking after the task cancel while the delegating turn ended normally, and the operation
  stayed `RUNNING`.
- `teamagents instances terminate --id … --yes` is the lever that stops work. It closes the instance's open
  execution (`close_epoch_execution`): a `PREPARED` operation is cancelled outright, an operation that
  already crossed its start boundary is *flagged* (`cancel_requested = 1`), and the driver that is waiting on
  that job turns the flag into a runner `CANCEL` — the runner kills the process group and the operation lands
  `CANCELLED` with a receipt whose error `class` is `cancelled`.

Measured end to end (`python3 review/dogfood/cancel.py`, five runs on 2026-09-26): the Leader's spawn turn
~5 s; the worker — which holds no `shell@workspace` until the user grants it (§5.1) — ran
`while true; do echo tick >> heartbeat.txt; sleep 0.2; done` under a real model; the user's direct input to
that member started it; `instances terminate --id … --yes` then stopped the effect **1.5–6 s after the
lever** (the heartbeat file is the witness: the process group is gone, not just a row), the operation ended
`CANCELLED` with `class: cancelled` — not the command's own `class: timeout` — and no operation was left in a
running state.

The probe's design rules, because each of them was needed to make the claims mean something:

- **the artifact decides.** The heartbeat file growing five times a second is the only thing the probe
  trusts; a database row would only show bookkeeping.
- **the receipt class distinguishes the levers.** `cancelled` means the user's lever reached the runner;
  `timeout` would mean the command simply ran into its own 120 s tool deadline. Without the class the two are
  indistinguishable from outside, and the probe would pass for the wrong reason.
- **the tick cadence must be faster than the sampling window.** With one tick per second a one-second quiet
  window can miss a tick and report a live process as stopped (D-84's rule); the command ticks every 0.2 s
  and three consecutive quiet half-second samples are required.
- **the model's part stays minimal.** The Leader only hires the worker (the one product path that creates a
  member with a real profile); the grant, the instruction and the stop all go through user surfaces
  (`authority grant`, the direct input the TUI sends, `instances terminate`).

Ceiling, and the open item this sharpens: there is **no light lever**. A user who wants to stop one running
command must retire the instance (and with it the member's future use); `cancel_operation` exists in the
control plane and is reachable over the documented socket protocol, but no CLI or TUI verb calls it. That is
new surface in the family of D-63's parked `cancel_turn`, so it stays a discussion item with the user rather
than something this probe implements. Two smaller observations from the same runs: `spawn_instance` never
grants `shell@workspace`, not even when the *user* issues it (by design — §5.1's automatic grant belongs to
`create_instance`), and a `spawn_instance` without a `profile` produces a member whose surface is a minimal
`wait`/`finish`/`read_history` (the Leader's `spawn` tool always passes a resolved profile, so the product
path is unaffected; a protocol client that omits it gets a member that can do nothing).

Evidence: `python3 review/dogfood/cancel.py` (five runs), and the offline layers it does not repeat —
`jobs_runner::cancel_running_stops_the_process_group`, `v2_driver::user_cancel_stops_a_running_job`,
`v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance`.

## D-87 `create = true` adopted a database that was not ours (2026-09-26)

`store::open(path, create)` is what the daemon passes, and its unstamped branch initialized the schema
wherever it ran. So a state root whose `session.sqlite` was **someone else's database** got the whole v2
schema written into it and was stamped `teamagents-v2`, while `doctor` refused the same file with "not a
v2 session database (no format stamp)". The claim in AGENTS.md and DESIGN's A34 — "a format stamp refuses
foreign or wrong-version databases" — held for the read path and failed on the path that actually runs a
session.

Measured before the fix (2026-09-26, a root holding a `users` table): `exec --state-root … "hi"` answered a
real model turn with exit 0, and the file afterwards carried `approvals … users … waits` plus
`meta.format_id = teamagents-v2`. Two things were wrong with that: another program's file was modified, and
the session then ran on a state root nobody had chosen.

The rule now is that identity is decided **read-only, before anything is written**:

- the stamp table is *read* (through `sqlite_master`), never created, and no pragma that writes (WAL,
  `synchronous`) is applied until the format check has accepted the file — so a refusal leaves the bytes
  exactly as they were;
- an unstamped file that holds tables the v2 schema does not create is refused by name, listing them:
  `… is not a v2 session database (no format stamp) and holds tables this session does not own (users);
  move it aside or use another state root`;
- the schema's *own* table names are still allowed through, because a crash between the schema batch and the
  stamp insert leaves exactly that state and it must be completed rather than stranded. The table list is
  derived from the `SCHEMA` text, so it cannot drift from it;
- a path that is not a SQLite database at all now says which file it is (`…: journal_mode: file is not a
  database` instead of a bare pragma error).

That makes the three entry points agree: `doctor` exits 1 with the FAIL line, `exec` refuses in 0.2 s with
exit 2 (infrastructure), and `init` refuses to prepare the root — all naming the file.

**New formal work.** A34's "migrate explicitly or refuse" claim was the last one in the acceptance matrix
with no model behind it (`verification/REPORT.md` recorded it as `—`), and this change is exactly the rule
that claim is about, so the identity decision is now a spec: `verification/tla/V2Store.tla` +
`MC_store.cfg` model an empty path, another program's database (with and without a format id of its own), our
own stamped file, our own file with the schema written and the stamp missing (the crash between the two
writes, reachable in the model through `WriteSchemaThenCrash`), and the read-only open. The claims are
`NoForeignAdoption`/`NoForeignStamp` (a foreign file is never stamped), `RefusalsWriteNothing` (a refusal is
silent, in `[][…]` form as `EveryRefusalIsSilent`), `InterruptedWroteOursOnly`, `AcceptedMeansStamped` and
the leads-to `HalfInitializedIsCompleted` (with the initializer weakly fair, an interrupted database is
completed rather than stranded). The counterfactual `MC_store_adopt.cfg` is this defect — `create = true`
initializing whenever there is no stamp — and `make verify-model-counterexamples` requires TLC to report
`Invariant NoForeignAdoption is violated`, which it does (2026-09-26). `make verify-model-all` covers
`MC_store.cfg` (48 states, 13 distinct, no error).

Evidence: `open_never_adopts_an_unstamped_file_that_holds_foreign_tables` (the refusal names the foreign
tables, and the file is compared **byte for byte** with what it held before),
`open_completes_a_session_database_that_lost_its_stamp_to_a_crash` (the deliberate crash-recovery
exception), and the live probe `python3 review/dogfood/boundary.py`, which also drives A33's second-daemon
refusal and the restart after a SIGKILL. Pre-fix control: with the foreign-table guard reduced to an empty
list the new test fails at its first assertion (checked 2026-09-26), and before the read-only reordering the
probe failed with `the foreign database was written to: tables are ['meta', 'users']`.

Ceiling: this refuses on *table names*, so a foreign file whose tables happen to be named exactly like the
v2 ones and which carries no stamp is still completed (it is then indistinguishable from our own
half-initialized database). The unstamped-but-v2 case is deliberate; a stamped foreign file is already
refused by format id.

## D-86 The public surface nothing calls, and the detector that names it (2026-09-26)

`clippy -D warnings` cannot see this class of defect: Rust's `dead_code` lint fires for private items only,
so a `pub fn`, `pub struct` or `pub const` that no caller anywhere mentions compiles clean forever. Two
earlier findings in this series came from hand passes (D-78's dead writers). The audit is now a command:

    python3 review/dead_code.py [--list-known]

It collects every public item definition in the three crates' `src`, their integration tests,
`engine/examples` and `engine/benches`, counts every other mention of each name in Rust files and in the
scripts and `Makefile` that drive the CLI, and reports the names whose only occurrences are their own
definition lines. Prose under `docs/**` and `review/**/*.md` is deliberately **not** a use — a name that
lives only in a document is documented, not called (the first version of this audit was fooled exactly
that way: the word "official" in DESIGN masked `Anthropic::official`).

First run over this tree: 409 public items, five of them uncalled. All five are gone:

| Removed | Why it was dead |
|---|---|
| `workspace::is_dirty` | a one-line wrapper over `dirty_status(cwd, false)`; both real call sites call `dirty_status` directly |
| `TaskStatus::is_terminal`, `OperationStatus::is_terminal` | no caller; the code matches the states where it needs them |
| `Anthropic::official` | a convenience constructor duplicating the config layer's own base-URL default (`providers/mod.rs` builds the provider from the catalog entry) |
| `theme::ZEBRA_BG` | a palette entry nothing paints; no table draws alternating rows |

The sixth finding was not deleted but made live: `REQUEST_KIND_TURN` was a constant with no reader because
its only comparison site compared against the bare literal `"turn"` (`import_response`, which refuses a
compression request). It now reads `REQUEST_KIND_TURN` there, so the turn kind has one name in Rust code;
the SQL keeps the literal, because a table default cannot reference a constant.

Six items stay uncalled on purpose, and the script prints each with its reason (`--list-known`), so the
allowlist cannot rot silently:

- `Envelope`, `ModelRequestRecord`, `Attempt`, `Operation` — DESIGN §4.1 names these row shapes as the
  minimal data contract. The running code reads them through SQL (`store.rs` owns the schema), so the typed
  form is the contract's representation rather than a call target. Deleting them would leave the
  `CREATE TABLE` text as the only statement of the contract.
- `workspace::member_worktrees` — belongs to the member-branch merge surface, which is an open item
  (D-76 and the gap recorded in `docs/ACCEPTANCE.md`); its sibling `merge_branch` looks used only because a
  test drives it, which is the detector's documented blind spot (a test counts as a use).
- `tools::wait_idle` — D-63's parked substrate for interrupt-and-redirect, carrying its own `ponytail:` note.

One documentation defect came out of the same pass: the doc comment on `driver::with_control` began with
"Stop driving; submitted commands stay committed (§4.1)", a sentence about the neighbouring method. It now
documents `shutdown`, which is what it describes.

Ceiling: this is a name-level detector, not a reachability proof. A Rust doc comment or a test counts as a
use, an item reached only through a trait object counts as used (it is called by name), and it knows nothing
about consumers outside the tree — this repository publishes no crate, so there are none today. It is
deliberately **not** wired into `make check`: it is a starting point for a human read, and a false positive
must not block a gate. Evidence: `python3 review/dead_code.py --list-known` reports 0 uncalled and 6 allowed
with their reasons; `make check` is green with unchanged counts (core 98 / engine 200 / tui 32) and `make
pty` passes after the deletions.

## D-85 The headline path finally has a model in it, and the needle can fail (2026-09-26)

Every real-model harness in this repository drove `exec`; the surface a user opens first — the TUI — had only
a *fake* daemon (`make pty`) and TestBackend frames. So the client half of a turn (composer → `submit_input`
frame → event-driven history refresh → rendering) had never met a real daemon and a real model in one run.
`review/dogfood/tui.py` closes that gap: it forks the real binary in a real PTY, waits for the session and
the leader on screen, types a prompt, presses Enter, and requires the answer to appear **on screen** before
it checks the session database for the same two entries and quits with Ctrl+C.

Design points worth keeping:

- **A screen needle must be the form the renderer paints, not the word the probe typed.** The first version
  looked for `TUIDONE`, which is *already* on screen before the turn: the word is part of the instruction
  sitting in the composer. It "passed" the answer in 1.1 s and proved nothing. The needles are now
  `i-leader TUIDONE` and `you Reply with the single word` — the labels `v2ui.rs` puts in front of an entry
  (the assistant's instance id, the user's `you`), which the composer echo cannot produce. The probe also
  carries a same-run positive control: the bare word must be on screen *before* Enter and the labelled one
  must not, otherwise the run reports that its own screen check is meaningless.
- **A fixed window is not a control; a measurement is.** The control's first version drained for a fixed
  0.8 s and then looked, and it failed on one run in three. Both causes were in the probe: `read_all` returns
  only when its own window closes, so the timings it printed were quantized to whole seconds, and the fixed
  window was too tight for the worst case — typing immediately after attach, while the client is still
  finishing its history/approvals/tasks/grants refreshes (measured 0.2–0.7 s for that first character). The
  control now waits up to 20 s and prints what it saw, and the drain steps are 0.25 s for screen waits and
  0.1 s inside the control. The separate, idle-machine measurement is the keystroke latency below.
- **One PTY reader.** The crashed first run exposed a latent bug: `tui/scripts/pty_screen.py::read_all`
  referenced `os` and `time` without importing them, and `pty_v2_smoke.py` carried its own copy of the same
  function. There is now one reader in `pty_screen.py` (imports fixed) used by the smoke, the new probe and
  this one; `make pty` stays green.

Measured 2026-09-26 (native windows, isolated state roots, one turn, DeepSeek Flash and Kimi k3-256k):

| Provider | TUI attached | Composer echo | Answer on screen | Total |
|---|---|---|---|---|
| `deepseek` | 2.4 s | 0.7 s | 2.7 s | 8.0 s |
| `kimi` | 2.7 s | 0.2 s | 2.5 s | 7.5 s |

The composer column is deliberately the worst case: the probe types as soon as the leader is on screen,
which is while the client is still finishing its attach-time refreshes (history, approvals, tasks, grants),
so the keystrokes wait behind them. Steady-state typing is the number below (0.03 s median).

The session database of both runs holds exactly the expected pair
(`{"role":"user",…}` and `{"role":"assistant","content":"TUIDONE"}` for `i-leader`), which is what the probe
asserts: not "text appeared somewhere", but *the client and the session agree about the same turn*.

The same investigation produced the client's own responsiveness, measured against the scripted daemon
(`python3 review/dogfood/input_latency.py`, no credentials, 2026-09-26): a single keystroke reaches the
composer with **min 0.01 s / median 0.03 s / max 0.11 s** latency, and ten characters written as one burst
render in 0.05 s. That is a product property nothing else measured, and it is the reason `tui.py` types a
whole prompt at once without reading "the last character took a while" as a defect. The instrumented probe's
guards are deliberately loose (1 s per keystroke, 0.3 s median, 2 s per burst) so a loaded machine reports
numbers instead of a red gate; `make pty` remains the deterministic interface check.

Ceiling: the probe drives one turn of the leader and no other member; the TUI's panels, approvals and task
cancellation against a real model still rest on the scripted smoke and TestBackend frames. It asserts on
screen text and the session database, not on pixels, so a layout regression that keeps the text visible would
pass. It is also not wired into any gate (it needs a credential and a network) — it is re-run by hand, and
the commands above are the whole procedure.

## D-84 Two of my own tests asserted traces that could not fail (2026-09-26)

The audits in this series keep finding "evidence" that proves less than it looks like, so I turned the same eye
on the tests added in the last weeks — the ones whose job is to pin A14, D-82 and D-83 — and found two
assertions that could not fail:

- `cli::an_unisolated_shell_refuses_instead_of_running_on_the_host` asserted that the workspace was empty after
  the refused `--check`. The command was `echo ran-unisolated`, which leaves nothing behind **even when it
  runs**, so the assertion held for the wrong reason. It now uses `touch ran-unisolated`, and the test carries
  its own control: the *same* command with the machine's `PATH` runs inside the sandbox (verdict `ok`, file
  present), and only then does the bwrap-less run assert the file's absence. Measured 2026-09-26: the control
  leaves `ran-unisolated` in its workspace, the refusal leaves nothing.
- `jobs_runner::go_past_the_deadline_is_refused_and_runs_nothing` asserted that a *relative* path
  (`late-go-ran`) did not exist, while the command's cwd is `/tmp` — the file would have landed in `/tmp`, and
  the assertion checked the test process's own directory. It now writes an absolute path inside the test's
  directory and asserts that file's absence.

Neither finding changes a product behaviour; both change what the evidence means. Stated as a rule for this
repository: **an assertion must be able to fail for the reason it names** — for a side effect, check a trace
the effect really produces (and, where cheap, run the positive control in the same test).

Ceiling: this was a manual pass over the newest tests, not a mechanical guard. A vacuous assertion is not
detectable by running the suite (that is the point) — the closest mechanical proxies are the ones this series
already uses: a counterfactual config for every model property (D-71/D-72's refuted controls) and a
pre-fix build for every regression claim.
## D-83 A gate flake with a precise cause: the fixture's own deadline (2026-09-26)

`make check` failed once in `jobs_runner::deadline_cancels_a_stuck_command` with

    thread '…' panicked at tests/jobs_runner.rs:130:28:
    go: "command is past its deadline"

and passed on the next run. The cause is in the fixture, not the product: `future(300)` is 300
**milliseconds** (the helper takes milliseconds), so the test asked for a deadline that its own setup had to
beat — two local-socket round-trips — and under a fully parallel suite those took longer once. The runner then
refused the late `go` with exactly the message this behaviour has ("command is past its deadline"), the test
read that as an unexpected error and failed.

**Product behaviour is right**: a `go` past the deadline starts nothing (the same family as A13's races), so
the fix belongs in the test — a 2-second deadline (≈7× the observed stall, and still cancels the fixture's
300-second `sleep` promptly) — and the refusal path now has a *deliberate* test instead of being covered by
accident: `jobs_runner::go_past_the_deadline_is_refused_and_runs_nothing` spawns a spec whose deadline is
already in the past, asserts the `go` is refused for the deadline, that the journal stays `READY` and that the
command left no trace.

Gate after the fix: `make check` three times in a row, green (24 test-target summaries each; core 98 /
engine 200 / tui 32).

Ceiling: this is the third fixture-timing defect of the same shape (D-79's database writer race, D-81's two
sites, this one) — a test that asserts a *timing* property ("it is cancelled by the deadline") must not race its
own setup to do it. There is no automated guard for that class; the lesson is written down here and in the two
other entries.
## D-82 Termination is final, and the surfaces say so (2026-09-26)

Walking the recovery path a user actually takes when a session has gone wrong turned up advice that cannot
work: `exec` refused a non-ACTIVE leader with one sentence for every lifecycle —

    the leader instance i-leader is TERMINATED; new input would not run.
    Resume it in the TUI instances panel (r) or use a fresh state root; nothing was submitted.

— but termination is **final**: the control plane refuses `set_lifecycle` on a terminated instance
(`daemon refused set_lifecycle: "instance i-leader is terminated"`, which is exactly what the CLI's own
`teamagents instances resume --id i-leader` prints), and the instance's workspace record has already been
retired (D-46/D-76). So "resume it" sent the user to a refusal; only the second half of the sentence was true.

**Three surfaces fixed**: `exec` now says the truth for a terminated leader ("termination is final (the
instance is retired), so this session cannot take new input. Start a fresh state root instead (--state-root
<new directory>)") while parked/paused keep the resume advice they deserve — and that advice now names a lever
the *caller* can pull, `teamagents instances resume --id i-leader` (D-68 added the verb; the message still
only offered the TUI key, which a headless run cannot use); and the TUI's instances-panel hint stops
advertising `p`/`r`/`t` when the selected member is terminated (the keys would answer with a refusal), keeping
`Enter`/`↑↓`/`Ctrl+N`/`Esc` — reading a retired member's conversation stays available.

Evidence: `v2_daemon::a_terminated_leader_is_reported_as_final_not_resumable` (the session refuses the resume
first, then `exec` exits 2 with the terminated wording and no "Resume"),
`v2_daemon::a_paused_leader_refusal_names_the_cli_lever` (a pause is not final, and the message names
`instances resume --id i-leader`) and
`tui::the_instances_hint_stops_offering_lifecycle_keys_for_a_terminated_member`.

Ceiling: a terminated instance is still *listed* (with its history readable) and nothing offers to revive it —
reviving a retired member (a new instance reusing the id, or a documented "un-retire") is a design decision
this entry does not make.
## D-81 Two more test-only second writers went through the product path (2026-09-26)

D-79's side note recorded one flake ("command receipt: database is locked") where a test wrote through its own
`Control` connection while the driver it had started was writing. A scan for the same shape — direct
`control.submit(...)` calls in tests that also run a driver or daemon — found the pattern at two more sites in
`engine/tests/v2_supervisor.rs` (`create_instance` while the supervisor runs in
`a_settled_goal_leaves_a_later_delegation_without_an_active_goal`, and `cancel_task` in
`a_prose_reply_leaves_one_turn_and_the_delegator_resolves_the_task`). Both now call
`SupervisorHandle::submit_user`, i.e. the same path the daemon and `teamagents tasks cancel` use, which removes
the second writer *and* tests the product's own client path instead of a side door. The rest of the flagged
sites are reads (`connection().query_row`, which never takes the write lock in WAL) or writes that happen
before their driver starts (the fixture pattern in the worker tests), so they stay as they are.

Ceiling: this is a test-side hygiene rule, not a product change — the product serializes its writers through
the daemon's single-writer worker (§4.1) and one coordinator per state root (A33) is what makes that
authoritative. A future test that needs a *second* writer can still do it; it should expect the busy error and
retry, or go through the supervisor.
## D-80 The address guard was cross-checked against its reference (2026-09-26)

`tools::guard_url`/`is_private_addr` are a hand-written port of Python's `ipaddress` policy (its own comment
says so), and a hand-written prefix table is exactly where one digit silently opens a whole block. The guard
now has a reference comparison to catch that: the block edges of both tables.

**The probe** (2026-09-26) generated cases with Python's `ipaddress` — every block edge in the guard's own
tables (`±2` around first and last address) plus 500 random IPv4 and 500 random IPv6 addresses — and compared
its verdicts with `is_private_addr`: **1224 cases, 1224 agreements, no disagreement**. The boundary half is now
a committed check, `tools::tests::the_address_guard_matches_the_reference_at_every_block_edge` (63 addresses the
guard must treat as non-public, 50 it must keep reachable), so a changed prefix fails there instead of in
production; the recipe is in the test's doc comment.

**Ceiling, stated honestly.** Two things this does not cover:

- the reference is Python's *policy* (what the guard claims to mirror), not a network-level truth: a block
  Python calls public and the local network calls private would still be allowed;
- resolution and connection are two separate lookups: `guard_url` resolves the host to decide, and the HTTP
  client resolves it again when it connects, so a name that changes between them (DNS rebinding) can still
  reach a private address. Pinning the checked address into the connection would need transport support the
  HTTP client here does not expose; a binding can opt into private targets deliberately with
  `env = { allow_private = "1" }`.

## D-79 The web tools say when they are absent, and their guard holds live (2026-09-26)

Verifying the last "basic tool" without live evidence turned up a small reporting gap of a familiar shape.
`web_fetch` and `web_search` are offered only when the config *declares* a binding — that part is the design
(§12.1: binding is the authorization, and D-74/D-78 kept to it) — but a config that declares none produced no
`doctor` row at all, so a fresh session gave the model neither tool while the README's feature list says "web
search and fetch". The user had no surface that said the capability was missing.

**The fix is one row**: `doctor` now warns when no web binding is declared, names the credential-free half
(`[tools.fetch]` with `kind = "web_fetch"` needs no API key) and what `web_search` additionally wants
(`provider`/`url`/`api_key_env`).

**And the tools now have live evidence**, which is what the check was for: `review/dogfood/web.py` runs two
turns in one session with a `[tools.fetch]` binding —

- the model fetches `https://example.com` and the page's **body** (not a snippet) reaches the conversation
  (`web_fetch` receipt carrying `Example Domain`), ending `completed`/`reply` with exit 0;
- the model is then asked to fetch `http://127.0.0.1:9/` and to report the tool's answer verbatim: the
  runtime refuses it (`{"error":"refusing private address for 127.0.0.1"}`), which is the SSRF guard
  (`tools::guard_url`) doing its job live — before, only `guard_url_blocks_private_targets` said so.

Measured 2026-09-26: deepseek 3.6 s (turn 1) with the guard refusing in turn 2; kimi 7.9 s (`reply="The
page's title is: **Example Domain**"`) with the same refusal.

Ceiling: `web_search` still has no live evidence here — it needs a search provider credential this machine
does not have, so its coverage remains the unit tests (`web_shapes_are_stable_and_bindings_are_required`, the
provider check in `web_tools`) plus `doctor`'s new rows. The fetch caps (2 MB, HTML only, readable text) are
documented and unit-tested, not verified against a hostile page.

**Side note from the same gate run**: `make check` failed once with
`v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart` → `history: "command receipt:
database is locked"`. The test's own `Control` connection *wrote* (a command receipt) while the driver it had
started was writing — a test-only second writer. The product serializes its writers through the daemon's
single-writer worker (§4.1), and a second process cannot boot a second coordinator at all (A33). The direct
read now happens after that driver stops, with the reason in a comment; the same run is green.

## D-78 The declared tool surface is reported, and removed surfaces leave nothing behind (2026-09-26)

The `pub fn`-with-no-caller sweep (D-74 ... D-77) came back to `engine` and found four things, one of which was
a reporting gap rather than dead code:

| Finding | What it was | What happened |
|---|---|---|
| `tools::validate_web_bindings` | a load-time check whose doc said "session.rs calls this while building the member's runner" - a module that no longer exists, so nothing did. The executor resolves `[tools.web]` **lazily**, so a typo'd provider or an unset credential surfaced only in a tool receipt, and `doctor` had no row for web bindings at all (it reports the MCP ones since D-74) | the gap is closed the other way: `doctor` now reports each declared web binding (kind, provider, whether its credential is set, whether the provider is one this build speaks) and a **FAIL** verdict naming the resolution the executor will perform when that fails; the function itself is gone |
| `config::save_custom_provider` | 59 careful lines (lock file, symlink refusal, `toml_edit` merge) that wrote a provider into the user config - the writer of the `/model` wizard D-40 removed | deleted, together with the now-unused `toml_edit` dependency; a "add a provider" surface would be new product surface and is not what the user asked for |
| `tools::shell_run_host` | a duplicate wrapper; the live shell path calls `shell_run_at` directly | deleted |
| `driver::cancel_turn` + `tools::TurnControl::wait_idle` | cancelling one turn (abort the stream, close the request) and waiting for the execution lock to free - the substrate of D-63's **open** question, "should the runtime interrupt a running turn instead of holding the input to the boundary?" | **kept**, each with a `ponytail:` note saying that no surface calls it, which live lever exists (`teamagents instances pause --id` stops the instance, not one turn) and that answering the question is what would wire them |

One list, one surface: `bound::DEFAULT_BINDINGS` is now the single place naming what a session binds
(`files`, `shell`, `web`, `skills`), and both the daemon boot and `doctor` use it - before, that literal lived
in `cli::daemon_boot` only, so a report had no way to describe the surface the session actually runs with.

Evidence: `cli::doctor_probes_isolation_and_config_errors` now drives three shapes through the real binary -
`[WARN] tools.search` with `api_key_env` unset ("reports a capability state instead of failing the session"),
`[ok  ] tools.search` once the variable is set, and `[WARN] tools.bad` ("provider \"nosuch\" is not one this
build speaks (anysearch)") next to `[FAIL] web tools` naming the same resolution failure for a `required`
binding. `make check` is green.

Ceiling: `doctor` *reports*; it does not fail the boot for an optional web binding, because the design says a
missing credential is a capability state at tool time (a `required` one still fails the member's start). And
the deleted writers are gone from the tree, not from history: `git log -S` finds them if a provider-editing
surface is ever wanted.

## D-77 The composer's history and word editing are wired (2026-09-26)

The dead-code sweep that produced D-74/D-75/D-76 looked at every `pub fn` with no caller, and
`tui/src/text.rs` stood out: its module header says "↑↓ recall history at the first/last row", the composer
keeps a 500-deep history with `record_submission`/`recall` (draft preserved, adjacent duplicates skipped) and
**has a unit test for exactly that** — and `v2app` called none of it. Nothing ever recorded a submission, so
the history was always empty; `↑`/`↓` scrolled the conversation instead (PageUp/PageDown and the mouse wheel
have always done that too); `Ctrl+←`/`Ctrl+→`/`Ctrl+W` (`move_word_left`/`move_word_right`/`delete_word`) and
`clear_composer` were unreachable. A first-class coding CLI whose composer cannot recall the prompt you just
sent is missing a basic affordance, and the module *documented* it.

**What is wired now** (nothing new was invented — the behaviour is the one `text.rs` already defined):

- a submitted prompt is recorded (`record_submission`) before the turn is sent;
- `↑`/`↓` walk a multi-line draft row by row and recall history at its first/last row, with the draft restored
  when you walk past the newest entry (the rule in the module header, now also in the footer hint: `↑ history`);
- `Ctrl+W` deletes the word before the caret, `Ctrl+←`/`Ctrl+→` jump by word;
- scrolling stays on `PageUp`/`PageDown` and the wheel (this is the one user-visible change: the arrows are the
  composer's now, which is also what the sibling CLIs do);
- `clear_composer` had no caller and no plausible binding, so it is gone rather than left as a dead affordance.

Evidence: `tui::the_arrows_recall_what_was_submitted` (two sends, then ↑↑↑ clamps at the oldest entry, ↓↓
restores the draft), `tui::the_arrows_walk_a_multi_line_draft_before_recalling` (the caret walks
`alpha\nbeta` first and the draft comes back with its newline), `tui::word_wise_editing_is_wired`
(`Ctrl+W` twice leaves `"alpha "`, `Ctrl+←`/`→` land on the word boundaries); and — the part a unit test
cannot reach — `make pty` now sends the terminal's own `↑`/`↓` escape sequences: the recalled prompt is
submitted a second time as a real `submit_input` frame, and `↓` back to the empty draft sends nothing (that
check fails with the recording removed). README's TUI key list and the footer hint describe the new keys.

Ceiling: the history is per composer (one session, in memory — it does not survive a TUI restart), and the
recall is textual: there is no search, no cross-instance history and no persistence. Whether the history
should live in the session state (so a restarted TUI still recalls) is a design question for the user.

## D-76 The workspace lifecycle, observed end to end (2026-09-25)

D-46 wired the §12.3 policies and promised, in the docs, that a terminated instance retires its workspace and
that uncommitted or unmerged work is never deleted — with unit tests and one driver test as evidence. Nothing
had observed the *whole* lifecycle with a real model, which is where the two interesting questions live: does
a member's tools really work inside its own worktree (the D-57 class: a dogfooding run once spent sixty turns
in the wrong tree), and what does the user actually do with a worktree's branch?

**The harness** `review/dogfood/workspace.py` builds a real git repository, has the model spawn a
`git_worktree` worker and delegate a file write to it, and then walks the lifecycle. Measured 2026-09-25:
deepseek 16.4 s / kimi 41.8 s, both `goal_status: SUCCEEDED`, the member's record showing
`policy: git_worktree`, and:

- the file the member wrote is **in its worktree and not in the shared project** (`git worktree list` names
  the checkout), so the isolation claim holds with a model in the loop;
- `teamagents instances terminate --id <id> --yes` succeeds while the work is uncommitted, the checkout is
  **kept**, and the daemon reports
  `workspace of <id> kept: the worktree has uncommitted, ignored or conflicting files; keep the results before cleaning up`;
- after the probe commits and merges the branch, the **running** session retires the checkout on its own next
  pass, record included (the retirement loop visits every TERMINATED instance, so a merge performed later is
  still picked up — a behaviour nothing tested before).

**One defect came out of it**: the retirement runs on *every* discovery pass, so that refusal was printed at
the poll rate (the harness produced five identical `daemon.log` lines in 1.5 s; a user who leaves an unmerged
worktree overnight would write ~50 MB). The supervisor now remembers the last reported refusal per instance
and reports the same reason once — a *different* reason, or a refusal after a successful retirement, is news
again (`supervisor::tests::a_workspace_refusal_is_reported_once_per_reason`). Re-measured: exactly one line
per reason, in the same scenarios, on both providers.

**Ceiling, and a gap this exposes**: the merge is the *user's* (or a Leader's, through `shell@workspace`),
because no surface merges a member branch: `workspace::merge_branch` and `workspace::member_worktrees` have no
caller anywhere in the tree (their doc comment calls the first one a "Leader-side merge helper"), and the
branch name is discoverable only from `<state root>/instances/<id>/worktree.json` or `git worktree list`.
That is recorded in ACCEPTANCE's known gaps: a `teamagents instances merge --id` verb (or a Leader-side merge
tool) is new product surface and needs the user's word before it lands.

## D-75 Config keys that did nothing now either work or say so (2026-09-25)

After D-74 the sweep continued over the config surface itself: every field of `UserConfig`, `ModelProfile` and
`ToolBinding` was checked against its read sites. Three keys had none beyond the `doctor` row and their own
parsing tests — accepted by the loader (`deny_unknown_fields` makes them *known*, so nothing complains) and
invisible to the runtime.

| Key | What it promised | What it did |
|---|---|---|
| `[permissions] mode` | "Full-auto must be user-chosen in config or CLI" (the code's own comment); `permission_mode_from_config()` exists with a dedicated error for a bad value | `cli::daemon_boot` built the mode from the flag alone, so a user who wrote `mode = "full_auto"` silently ran in `approved_scope` — the safe direction, and still a lie: their out-of-scope calls parked on approvals they had switched off. The helper had **no caller** anywhere |
| `[retention] archived_days` / `history_days` | "Delete archived sessions untouched for this many days when a session is opened" / "Drop applied deliveries and events older than this many days" (the struct's own comments; DESIGN §9 promises ordinary history is "archived or cleaned per user configuration", while live references and evaluation evidence are never evicted) | nothing: no code path archives, prunes or deletes, and `doctor` printed `[ok ] retention archived_days=30 history_days=7`, which reads as "in effect" |
| `models.*.codex_profile` | layer `$CODEX_HOME/<name>.config.toml` through `codex --profile <name> app-server`, so the Codex profile owns provider, model and credentials | nothing, and DESIGN Q12 excludes an external Codex adaptation from this release; a config carrying it ran the shipped provider while the user believed Codex owned the credentials |

**What each one gets, and why it differs:**

- **`mode` now works.** The user's config is the session default and `--full-auto` still asks for host
  execution for one boot; a project file can never set it (`permission_mode_from_config` reads the user config
  only), so D-41's "user-only" rule is intact and the surface matches the sibling CLIs the user pointed at
  (their sandbox/approval policy lives in the config file). Evidence:
  `cli::full_auto_reaches_a_started_daemon_and_is_reported_against_a_live_one` now boots three sessions —
  config `full_auto` → the greeting says `full_auto`, config `approved_scope` → `approved_scope`, flag over
  config → `full_auto`. With the pre-fix line restored the same test fails (`left: "approved_scope",
  right: "full_auto"`).
- **`retention` is reported, not implemented.** Deleting history is destructive and the design ties it to
  conditions that need their own verification (ordinary history may be cleaned; live references and
  evaluation evidence may never be evicted), so the honest step now is that `doctor` stops implying it works:
  the row is a WARN saying the numbers "are not applied: this release never archives or prunes a session, so
  nothing is deleted". Implementing it is the user's call and is recorded in ACCEPTANCE's known gaps.
- **`codex_profile` is refused.** A key that changes which provider and credentials a member uses must not be
  ignored; the loader now fails with `models.<key>.codex_profile = …: an external Codex profile is not part of
  this release; configure the member directly with provider/protocol/base_url/api_key_env`
  (`config::a_codex_profile_is_refused_instead_of_ignored`). The field stays declared — with its comment
  corrected — so a future release can implement it deliberately.

Ceiling: this closes the three keys that existed, not the class. The rule it restates (and the reason each
case is handled differently) is: **a config key this build does not serve is either made to work, refused
with a pointer, or reported as not in effect — never accepted in silence.**

## D-74 A configured MCP service is bound by declaring it (2026-09-25)

The audit of A25 ("MCP approval / cancellation / unknown outcome", D-25) asked a question the tests could not
answer: how does a *user* bind an MCP service? The protocol side is well covered (stdio and streamable HTTP
through fake servers, the approval/cancel/unknown-outcome paths), the design lists MCP as a shipped feature,
the user guide documents `[tools.web]` as the "optional tool binding (web / fetch / mcp)" section — and the
answer was: **you cannot**.

`BoundTools::load_in` loads a service when its *name* appears in the member's bindings list. That list is
built by the product — `cli::daemon_boot` passes `["files", "shell", "web", "skills"]` and nothing else
anywhere in the tree — so an entry like

```toml
[tools.probe]
kind = "mcp"
command = "/usr/bin/python3"
args = ["-u", "probe_server.py"]
```

was parsed, validated by `doctor`, loaded into the merged (trust-filtered) catalog, handed to every driver…
and never selected. The probe makes it visible: a server that records its own start writes nothing, because
it is never spawned — no error, no warning, just a capability that is not there. (The web half of the same
section has always worked, because a `web_search`/`web_fetch` entry is selected whenever `web` is bound; MCP
was the only kind that needed a name no surface could provide.)

**The fix is one rule**: a `[tools.<name>] kind = "mcp"` entry in the merged catalog *is* the user's binding
of that service, exactly as it already is for the web tools. The catalog that reaches the loader is the
user config merged with the project config under the documented trust rule (`[permissions]
trust_project_tools = true`, or the project's tools are dropped — `config::load_user_config_for`), so a
cloned repository still cannot bind anything on its own. Names in the bindings list keep working (an unknown
or unsupported one is still refused), and a service marked `required` still fails the member's start loudly —
which now means a mistyped `command` in a `required` entry stops the session at boot.

**`doctor` reports each declared service first** (the D-66 rule: a config surface a user cannot see is a
trap): a row per `[tools.*]` MCP entry saying whether its command is runnable (or its http url present), that
it runs over which transport, and whether it is required. A `required` service with an unrunnable command is
now visible before the session boots rather than only in `daemon.log`.

Evidence (all re-runnable):

- `bound::a_declared_mcp_service_is_bound_without_naming_it_in_the_bindings` — the product's own bindings
  list plus one declared service: the bound tool appears (`probe_probe_ping`, i.e. `<service>_<tool>`) and is
  the schema the model call advertises, the server really started, and a name the catalog does not define is
  still refused. With the pre-fix rule restored the same test fails (`left: [], right: ["probe_probe_ping"]`).
- `v2_supervisor::a_configured_mcp_service_reaches_the_members_surface` — the leader's *offered* tool list
  from a real session carries `probe_ping` next to the built-ins (fails pre-fix: the surface is
  `["shell","wait","send","delegate","spawn","finish","read_history"]`).
- `cli::doctor_probes_isolation_and_config_errors` — the new rows: `[ok ] tools.good`, `[WARN] tools.typo`
  ("not runnable"), `[ok ] tools.remote`.
- Real models, both protocols: `python3 review/dogfood/mcp.py [--provider kimi]` — the server's log shows
  `initialize`/`tools/list`/`tools/call`, and the run reports the tool's own output (a token the server
  generates at start, so a model that answered from the tool's *description* instead of calling it cannot
  pass): deepseek 2.5 s, kimi 9.6 s, both `end=reply` with the token.
- The pre-fix behaviour, measured: a real session with the same config started the daemon and never spawned
  the server (no marker file, no `tool service` line), while the model's request went out without the tool.

Ceiling: a declared service is bound to **every** member (the bindings list is per session, not per member) —
per-member MCP selection would be new surface and has no user request behind it. And an MCP server is started
with the environment the tool gateway builds (its own `env` table plus the documented references), not with
the daemon's ambient environment — worth knowing when a server expects a variable to be inherited. No new
formal claim came with this change: an MCP tool reaches a model only through `BoundTools::schemas()`, whose
only inputs are the trust-filtered catalog and the bindings list, so the design's "binding is the
authorization" (§12.1) still holds by construction, and `V2Grants::OfferedToolsAreAuthorized` continues to
cover the grant-backed half of the offered surface.

## D-73 The CLI refuses what it does not honour (2026-09-25)

The documented-surface audit that produced D-71/D-72 turned to the entry points themselves, and found the same
class of defect in the argument parser: **arguments accepted with nothing behind them**.

| Input | What happened | What happens now |
|---|---|---|
| `teamagents hello` | `hello` was parsed as a positional, the top-level dispatch fell through to `run_tui`, and a session was booted with the word silently dropped — a typo'd verb (`teamagents exex "…"`) or a pasted prompt lost exactly what the user meant | exit 2, the message names the word, says the TUI takes no prompt, and points at `teamagents` / `teamagents exec "…"`; nothing is started |
| `teamagents frobnicate` | same fall-through (a stray word is not a verb) | same refusal |
| `teamagents -v` / `--verbose` | the flag was parsed into a field no code ever read: verbose logging exists in no release this binary serves | exit 2, pointing at `<state root>/daemon.log` and naming `--version` (the plausible `-V` typo) |
| `teamagents-tui --cwd DIR` | the TUI accepted `--cwd`, `--full-auto`, `--resume` and `--team` and honoured none of them (the engine passes the socket and the state root; the session's workspace and mode belong to the daemon) | exit 2 with the flag named and the pointer to `teamagents --cwd DIR` / `teamagents --full-auto`; the engine no longer passes `--cwd` through |
| `teamagents` on a machine with no config | the daemon refused with `use --model to name a catalog profile (available: )` — a first run misreported as a missing flag | the message names the step that creates a catalog (`teamagents init`, then `doctor`) |

The rule this restores is the one the upgrade notes already state for the *removed* entry points
(`--plain`/`--resume`/`--team`, `validate`/`sessions`/`serve`/`repl`): an argument this binary does not serve
fails with a clear message instead of being ignored. It matters more here than it looks: the TUI path has a
**side effect** (it boots a daemon and opens a session), so silently dropping an argument also wasted the
user's session on the wrong work.

Evidence: `cli::a_bare_word_and_verbose_are_refused_without_starting_a_session` drives the real binary for
`hello`, `frobnicate` and `-v` (exit 2, the message names the input, and — the part that matters —
`<state root>/daemon.sock` was never created); `tui::cli_flags::the_tui_refuses_the_flags_the_daemon_owns`
drives the real front-end for `--cwd`/`--full-auto`/`--resume`/`--team` and shows a supported invocation still
parses. `make pty` keeps driving the real terminal through the engine (which no longer passes `--cwd`), and
`make check` is green.

Ceiling: `--help` after a *known* verb still prints the global help rather than per-verb help (the usage
lines are in it, so nothing is misleading); and the two flags of the removed `sessions` verb (`--dry-run`,
`--history-days`) are still parsed so that `teamagents sessions prune --dry-run` reaches the "no longer
supported" pointer instead of a bare usage error.

## D-72 A run reports its own input's outcome, also when that input waited (2026-09-25)

D-71 taught the client to stop reading the runtime's word as the member's answer. The next audit of the same
surface — the D-63 path where `exec` arrives while the leader is already in a turn — found the same defect one
step earlier, and this time in the *primary* outcome:

```
$ teamagents exec "second question"      # submitted 1 s into the first run's slow turn
exit=0 end=completed goal=SUCCEEDED reply=null input_queued=true
```

The prompt was queued (D-63 reported that honestly), the *first* run's turn then settled the goal, and `exec`
reported **that settlement as its own outcome**: exit 0, "completed", no answer to the question it was given.
The same shape applies to a plain reply: while the queued input waited, the earlier turn's reply was the
newest assistant entry, and a poll that landed in the boundary between that reply and the drain reported it
as this run's answer. D-49's contract says "own outcome only"; it was written for a settlement left by an
earlier *run*, and the D-63 queueing path had no equivalent rule at all.

**The rule.** A turn-ending entry is this run's outcome only if it comes *after* this run's own input entry in
the conversation. `exec` knows that entry exactly, because it generated the envelope id, and the daemon's
history view now names it (`envelope_id`, D-72 also fixes the page's order to (epoch, idx), which is the order
the conversation actually has across a reset). From one history snapshot the client takes its own position and
then reads the **first** turn-ending entry after it — the member's text (no pending tool calls) or the
runtime's closing word: that is its answer, and a later turn's entries are not. A settlement is recognised by
the runtime's own note (`goal-close-*`, `goal-block-*`), whose position after this run's entry is exactly the
fact "it happened after my input landed" because the note is appended in the settlement's transaction.

**What the rule is allowed to assume, and why it is verified.** A queued input is applied at a READY boundary
and the next request is fixed from that context (§5.3), so a turn begun after the landing *contains* the
input, and an outcome recorded before the landing cannot belong to it. That premise is a model property, not a
hope: `InputLandsAtTheBoundary` (D-63) plus the boundary's own rule that a request may not begin while the
inbox holds something (`~inst[i].queue`, whose counterfactual `MC_control_midturninput.cfg` is refuted). D-72
adds the property that names the consequence: **`SettlementFollowsATurnAfterTheLanding`** — at settlement time
a request must have begun since the instance's last input landing. It is refuted by the same
pre-D-63 counterfactual in its own control (`MC_control_landing.cfg`, `AllowMidTurnInput = TRUE`), whose
counterexample walks `BeginRequest → MidTurnInput → RecordAttempt → ImportResponse → SettleGoal` and shows the
settlement of a turn that never saw the input — precisely the outcome a waiting client would have attributed
to it. The model also gained the abstraction it was missing for this: `SettleGoal` now requires
`tail = "assistant"`, i.e. a settlement is about the model's own completion, which is what both code paths do
(`complete_goal` reads a decision's candidate; a runtime block follows the check round of one).

**An input that will never land says so.** A queued envelope whose epoch closes before the boundary reaches it
is sealed as `SUPERSEDED` (§5.3/A24) — a reset sealed the one in this audit's probe. The runtime names what it
seals (`envelopes_sealed`, from both places that seal: the drain dropping a stale-epoch leftover, and
`close_epoch_execution` closing an epoch), and `exec` ends at once with the new terminal
`end: "undelivered"` (exit 1) instead of letting the caller wait out its own deadline and then call a dropped
input a timeout.

**Evidence** (all re-runnable):

- `v2_daemon::a_queued_input_is_not_answered_by_the_previous_turns_settlement` — a real socket, a slow
  settling turn, the second input queued behind it: the run reports `end: reply` with **its own** answer,
  `goal_status: null`, `input_queued: true`, exit 0, while the session's goal really is `SUCCEEDED`. With the
  pre-fix decision restored the same test fails (`left: Completed, right: Reply`).
- `v2_daemon::a_queued_input_is_not_answered_by_the_previous_turns_reply` — the positional half of the rule.
- `v2_daemon::a_queued_input_a_reset_sealed_is_reported_undelivered` — a reset while the input waits: exit 1
  with nothing claimed as its outcome, instead of a 30 s wait for a timeout.
- `v2::exec::tests::an_outcome_before_the_runs_own_input_is_not_its_outcome` — the rule as a pure function
  over the shapes a conversation can have (unlanded input, earlier settlement, tool traffic, a later turn,
  the run's own settlement, a closed turn, a bare tool call).
- Real models, both protocols: `python3 review/dogfood/queued_input.py [--provider kimi]` — run 1 settles its
  own goal, run 2 is queued inside it and reports `end=reply` with its own word (`BANANA`) and
  `goal_status: null` (deepseek 2.8 s, kimi 40.4 s). Against the pre-fix build the same harness fails with
  `end=completed / goal=SUCCEEDED / reply=null` for the queued run.
- Formally: `SettlementFollowsATurnAfterTheLanding`, listed in `MC.cfg` and `MC_control_two.cfg`
  (`make verify-model-all` green: MC.cfg 84,877 states, MC_control_two 958,777), and refuted by
  `MC_control_landing.cfg` in `make verify-model-counterexamples` (now 9 controls, each refuted).

Ceiling, stated honestly: the client's rule is positional over the conversation, so it inherits the history
page's bound (`exec` reads 400 entries) — a run whose input entry has already fallen out of the page cannot
be attributed, and `exec` then reports a timeout rather than inventing an outcome. The queue is still
per-instance and per-epoch: an input sealed by a reset is *reported*, never re-delivered, and re-sending it is
the caller's decision. And the *session-level* facts stay ungated on purpose: a permanently failed leader
request and a pending approval are reported even when they belong to the turn the input waited behind, because
they are why this run cannot deliver anything — the report carries the reason and the exit code is non-zero
(1/3), so nothing is claimed as this input's own outcome.

## D-71 A runtime-blocked goal is not a reply: the runtime's own word is never the member's (2026-09-25)

A16's harness says a required check that can never pass must end the run **failed with the goal BLOCKED**.
Against DeepSeek that is what happened (D-70). Against the second catalog entry, Kimi over the `responses`
protocol, the same scenario ended

```
provider=kimi exec exit=0 end=reply goal=None | goal BLOCKED | check rounds 3 | repairs 2
```

— the goal *had* been blocked, and the headless run reported a **success with no goal status**: a false
success in the exact place the gate exists to prevent one. Two defects compounded:

1. **The block was not a settlement.** `complete_goal` announces its outcome with a `goal_completed` event;
   `block_goal` wrote the goal status and emitted only `goal_blocked`. A client that follows a goal's ending
   through the event log — the headless run, the TUI — never learned that the goal ended, so `exec` fell
   through to "the last entry is the reply".
2. **The runtime's own closing note was stored as the member's.** All three runtime notes
   (`runtime: goal … blocked: …`, `runtime: turn closed`, `runtime: goal … closed as …`) were context
   entries of kind **`assistant`** with an assistant-role message. The client's last resort — read the last
   assistant entry as the model's answer — therefore returned the runtime's *own* sentence
   (`runtime: goal goal-s-main blocked: required checks failed (impossible:exit) after 3 round(s)`) as the
   reply, and `exit 0`. The note was written that way on purpose (a note that looks like model text keeps
   the driver idle), which is exactly why the confusion was invisible: the code had one word for two
   speakers.

**The fix, in three places, each with the reason it is the right level:**

- `block_goal` emits the same `goal_completed` event as `complete_goal`, with `status: "BLOCKED"` and
  `blocked_by: "runtime"` (the distinct `goal_blocked` event stays: it carries the check names and reason,
  and keeps "who decided" auditable). A settlement is a fact of the event log, not only of a snapshot a
  polling client happens to read at the right moment.
- The runtime's closing notes get their **own entry kind** (`EntryKind::Runtime`, `kind = 'runtime'`) in the
  **user's voice** (`role: "user"`, the same voice the existing `note` kind uses for runtime-authored
  facts). The kind is the discriminator every reader already uses (`exec` reads entry kinds, the TUI switches
  on them), so no client has to pattern-match `"runtime: "` text to tell the runtime apart from the model.
  The idle rule counts the committed tails — the model's own text **or** the runtime's closing word — as
  answered, which is what the assistant-shaped note was for: a runtime that re-opened a turn against its own
  settlement would be inventing work out of its own ending (formally: `RuntimeTailIsWork`).
- `exec` reports what actually happened. A turn the runtime closed with **no settlement this run can claim**
  (the goal settled in an earlier run, or the `finish` had nothing left to settle) is the new terminal
  `end: "unsettled"`, exit 1 — not a `reply` whose text the member never said, and not a timeout that would
  mislabel a finished turn. The loop reads the checkpoint **before** the events for this: a settlement
  commits its event and the phase change in one transaction, so that order cannot see an idle instance whose
  settlement is still unread.

**Sessions written before the fix are migrated, not left behind.** Schema 2 → 3 rewrites exactly the three
runtime envelopes (`goal-close-*`, `goal-block-*`, `turn-close-*`; a model entry carries its decision id
there, never one of these) from `assistant` to `runtime` with `role: "user"`. The migration walks any older
store up step by step in one transaction (a `1`-stamped store still reaches the current version), which the
previous one-step rule did not allow. Probe on a real pre-fix root (`/tmp/ta-providers-run`, written by the
previous build): before `schema_version = 2`, `i-leader:0:13 kind=assistant {"role":"assistant", …}`; after
one daemon boot on the new build, `schema_version = 3`,
`i-leader:0:13 kind=runtime {"role":"user","content":"runtime: goal goal-s-main closed as SUCCEEDED"}`.

**Evidence** (all re-runnable):

- `v2_daemon::a_runtime_blocked_goal_is_not_reported_as_a_reply` — the whole scenario through a real socket
  (a check that always fails, three repair rounds): `end: failed`, `goal_status: BLOCKED`, `reply: null`,
  exit 1, the `goal_completed` event present with `blocked_by: runtime`, and the instance's tail entry of
  kind `runtime`.
- `v2_daemon::a_turn_closed_by_the_runtime_without_a_settlement_is_not_a_reply` — the second run on a settled
  goal: `end: "unsettled"`, exit 1, returned at once instead of waiting out its 30 s deadline.
- `v2_driver::required_checks_exhausted_parks_the_goal_blocked` (updated: the block *is* a `goal_completed`
  settlement now), `core::v2::control::closing_a_turn_answers_its_finish_call` (the close marker is a
  `runtime` entry in the user's voice), `v2::store::migrate_rewrites_the_runtimes_closing_notes`,
  `tui::the_runtimes_closing_note_is_not_the_members_message`.
- Real models, same harness, both protocols: `python3 review/dogfood/checks.py --provider deepseek`
  (exit 1, `end=failed`, goal BLOCKED, 11 requests, 9.7 s) and `--provider kimi` (exit 1, `end=failed`, goal
  BLOCKED, 8 requests, 37.8 s) — where the Kimi run had reported `exit 0 / end=reply / goal=None` before.
- `python3 review/dogfood/providers.py` (A27, two providers) still completes: exit 0, goal SUCCEEDED, the
  delegated task SUCCEEDED, both members on their own model, 8 requests, 23.1 s.
- `python3 review/dogfood/runtime_note.py --providers deepseek,kimi` — the note is not only *stored* under
  its own kind, the transcript still works: two turns in one state root, the first settling `SUCCEEDED` and
  the second (whose request carries the runtime's user-role note) answering normally with exit 0. deepseek
  1.6 s then 0.9 s, kimi 8.9 s then 11.8 s, the note at `i-leader:0:4` as `runtime`/`role: user` in both.
  This is the check the shape needed: `materialize` sends `entry.message` verbatim, so the change had to be
  safe on the DeepSeek thinking wire and on Kimi's `responses` wire, and it is.
- Formally: `NoTurnWithoutWork` now says *no turn while the tail is committed* (the model's text or the
  runtime's closing note), `SettleGoal` leaves `tail = "runtime"`, and the new negative control
  `MC_control_runtimeTail.cfg` (`RuntimeTailIsWork = TRUE`) is **refuted** by that invariant with
  `tail = "runtime"` and `phase = "MODEL_PENDING"` in the counterexample state. `make verify-model-all`
  (10 configurations), `make verify-model-counterexamples` (8 controls, each refuted) and
  `make verify-kani` are green.

Ceiling, stated honestly: the kind and the voice are the fix for *new* sessions and the migration covers the
old ones, but a client that keys on text rather than kinds would still be reading a convention. The Kimi
harness also showed the D-65 ceiling from the other side, unchanged by this decision and worth knowing: when
the *worker* answers with prose instead of calling `finish`, the delegated task stays RUNNING, and a leader
that verifies the artifact itself (as it did here — it read the worker's response and the file) can settle
the goal SUCCEEDED with that task still open, because `complete_goal` checks open **operations**, not open
**tasks** — which is what §4.2's completion transaction names, with the delegator's own `wait` as the
mechanism that should hold a leader until its delegated work resolves. Whether the runtime should refuse such
a settlement anyway is a design question about team semantics for the user, not a defect this entry fixes.

## D-70 The completion gate works on a thinking-mode provider (2026-09-25)

Closing A16's last gap — a *real-model* run whose required check fails — found a defect that no deterministic
test could: the check round's own conversation entry is a **runtime-authored assistant message with tool
calls** (`register_check_runs` appends `"runtime required-check round N for goal …"` plus the shell calls,
because the check outputs must answer a tool call to be wire-valid), and DeepSeek's thinking mode requires
such a message to carry `reasoning_content`. It did not, so the *next* request — the repair turn that the
completion gate opens after a check fails — was rejected outright:

```
chat API 400: The `reasoning_content` in the thinking mode must be passed back to the API.
```

In other words: on the default provider, **any goal whose required check failed died on the wire instead of
being repaired or blocked** — the gate was unverifiable in exactly the case it exists for. A16's row had
recorded an earlier wire error there (D-54 fixed the unanswered `finish`); this was the second half.

**Pinning the rule with a replay probe** before touching code (same session, same messages, four variants
sent to the API): as the failed run had it → `400 reasoning_content …`; with a labelled filler on the
synthetic entry → `200`; without the synthetic assistant message (orphan tool result) → `400 Messages with
role 'tool' must be a response to a preceding message with 'tool_calls'`; with no reasoning anywhere →
`400`. So the transcript shape is forced by the wire (the synthetic call must exist), and the missing field
is the defect.

**The fix lives in the adapter, not in the stored context**: `ChatCompletions::with_reasoning_echo` (set for
the `deepseek` protocol in `build_for_model`) fills an **empty** `reasoning_content` on assistant messages
that carry tool calls and have no recorded reasoning. An empty value satisfies the requirement and invents
nothing; the probe also verified it is accepted by `deepseek-flash`, `deepseek-reasoner` and `deepseek-chat`,
so the scope is safe for every model on that protocol, and a *recorded* reasoning is never overwritten.
Keeping the field out of the context preserves the design's protocol-neutral kernel (native continuation
fields stay per-protocol, as the existing `responses_output`/`anthropic_blocks` stripping already does).

**Verification**: `providers_fake::the_thinking_wire_echoes_reasoning_for_assistant_tool_calls` (the filler
appears only when the flag is on, and a recorded reasoning is untouched), and the real-model harness
`review/dogfood/checks.py` that found the defect now completes: 8 model requests, 12.7 s, `end=failed`,
goal **BLOCKED**, the artifact written, the repair ledger naming `check_id: impossible` / `class: exit`, and
the model itself reporting that it would not bypass the gate.

Ceiling: the *protocol* decides the echo (`deepseek`), not the model name; a non-thinking DeepSeek model on
that protocol receives an empty field it ignores (verified), and a thinking model served over the plain
`openai` protocol would need the same treatment — the swap is the flag.

## D-69 Which model a member runs on is written down and visible (2026-09-25)

Making the two-provider acceptance row (A27) re-runnable in the tree — a real DeepSeek + Kimi session, below —
surfaced a visibility hole. A spawned child stores its profile when it is created (D-59 made that the
*resolved* model name, so the factory can map it back to a catalog key), but the **leader's** row was created
by the bootstrap with no profile at all: `instances.profile_json` was `{}`, and no surface reported a model
anyway. For a team that deliberately spans providers (`spawn(model = …)`) that means the user cannot see who
runs on what — in the TUI, in the daemon's snapshot, or in the CLI listing.

**The fix** (three small pieces, no new mechanism):

- `driver::bootstrap` stores the leader's resolved profile (`model`, `instructions`, `options`,
  `context_window`) on the instance row, so *every* instance row describes its model. The leader's driver
  still takes its profile from the session configuration — the row is what a reader has. The bootstrap's
  idempotence is unaffected: it only creates the row when the instance does not exist yet, so a fixed
  `boot-instance` command id never replays with a different payload (the check happens before the submit).
- The daemon's `checkpoint` snapshot carries `model` per instance (a read-view extension, like D-61's grant
  view), so **every** client sees it.
- The TUI instances panel and `teamagents instances` print it (`i-worker · ACTIVE · READY · k3-256k`).

**Evidence**: `v2_daemon::the_snapshot_reports_each_members_model` (the row and the snapshot carry the
*resolved* name `deepseek-flash`, not the catalog key `leader_main` — consistent with what a spawned child
stores) and `tui::frame_shows_the_panels_and_panel_hit_testing` (the panel renders each member's model).

**The re-runnable A27 harness** is `review/dogfood/providers.py`: one session, the Leader on DeepSeek Flash
(native 1M window, D-36) and a worker spawned with `model = "worker_kimi"` (the user's Kimi entry, 262,144
tokens), delegating a file write and waiting for it. Measured (2026-09-25, isolated state root): 7 model
requests, 14.0 s, `end=completed`, goal `SUCCEEDED`, members `i-leader` on `deepseek-flash` and `worker1` on
`k3-256k`, `task t1 SUCCEEDED`, and `answer.txt` exactly as asked. A27's row now cites this script instead of
an out-of-tree probe.

## D-68 The user-side interventions are reachable headlessly (2026-09-25)

D-65's honest ending for a model that stops talking — the delegator waits, and **cancelling the task** releases
it — was reachable only from the TUI, and the same is true of the other §5.4 levers: `exec` tells a stuck user
to "resume it in the TUI instances panel (r)", and the operating notes send the user to the tasks panel to
cancel a task that can only wait. A headless or CI user has no panel, so the documented recovery paths were
unreachable for exactly the users `exec` exists for.

**`teamagents tasks [list] [--json]`, `tasks cancel --id ID`** and
**`teamagents instances [list] [--json]`, `instances pause|resume|terminate --id ID [--yes]`**
(`engine/src/v2/intervene.rs`) close that: they are clients of the ordinary user commands
(`cancel_task`, `set_lifecycle`) whose identity rules the control plane already enforces — *the user controls
every transition; the system may only park* (§5.4) — with the same exit codes and prefix resolution as
`authority`/`approvals`.

Two deliberate choices: `terminate` requires **`--yes`**, because termination retires the instance's
workspace (a directory with uncommitted or unmerged work is never deleted — only reported) and the TUI asks
for a confirmation for the same reason; and the ids must name a *listed* instance or task, so a typo is a
client-side refusal instead of a guess about something else.

Evidence: `v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance` drives the real
binary against a real daemon through the whole D-65 flow — the worker's prose leaves the task `RUNNING` and
the leader `WAITING`, `tasks` lists it, `instances pause`/`resume` move the lifecycle, `terminate` is refused
without `--yes`, `tasks cancel` cancels it, and the delegator wakes and settles the goal `SUCCEEDED`. That
test is also the correspondence for a wait fact D-65 depends on: a `CANCELLED` task *satisfies* a delegator's
task wait (the code's task condition accepts `SUCCEEDED|FAILED|CANCELLED`; a `BLOCKED` task does not).

Ceiling: no bulk levers (`cancel --all`, `pause --all`) and no "send a message to a worker" verb — each
action stays one named subject, and the TUI remains the surface for browsing a large team.

## D-67 Approvals are reachable headlessly (2026-09-25)

Dogfooding the documented first run (`init → doctor → exec`) in a clean `HOME` with a real model ended at a
dead end: the leader's first out-of-scope call parked the session on an approval, `exec` reported
`end: approval_required` and exited **3** — and its message said the only ways on were the TUI or
`--full-auto`. For a *headless* user (the reason `exec` exists, and what a CI job can actually run) that is a
wall: the session is parked, the TUI is not available, and `--full-auto` changes the permission mode of the
whole session.

The mechanism was never missing — this is D-61's shape once more. The daemon's `approvals` read already
carries the **id**, the operation, the tool and a bounded preview (so a client could always have decided,
unlike the grants view D-61 had to extend); `approve`/`deny` are ordinary user commands; and the decision is
bound to the operation and its argument hash, so a modified call needs a new decision (§6.2). The gate itself
is verified: `V2Control::NoEffectBeforeApproval` (± `A25`'s tests) — what was missing was a client.

**`teamagents approvals [list] [--json]`, `approvals approve --id ID`, `approvals deny --id ID`**
(`engine/src/v2/approvals.rs`): a client of the same socket, taking the full id or an unambiguous prefix
(the resolution rule now lives once, in `exec::resolve_prefix`, shared with `authority`), with the same exit
codes as `authority` (0 done, 1 the session refused it, 2 usage/no session). The listing names the exact call
being approved; a deny prints that the operation fails closed.

Real-model evidence (one shell, clean `HOME`, the shipped config, DeepSeek Flash): `exec` parked on
`printf hi > shellproof.txt; echo "exit=$?"; …` → `teamagents approvals` listed
`ap-d-req-4caeb0c6-…:0  shell  printf hi > shellproof.txt; …` → `approvals approve --id ap-d-req-4caeb0c6`
→ the session dispatched the approved call, the **goal reached `SUCCEEDED`**, `shellproof.txt` contained
`hi`, and the list was empty afterwards. Deterministic evidence:
`v2_daemon::the_approvals_cli_lists_and_decides_a_parked_operation` drives the real binary against a real
daemon socket (listing, prefix decision, a typo refused as a client error, the goal completing after the
decision).

Ceiling: the decision is still one *visible* call at a time — there is no "approve everything like this" rule
and no interactive prompt, because the design binds a decision to one operation and its arguments (§6.2); a
user who wants unattended runs still chooses `--full-auto` deliberately.

## D-66 The doctor reports the skills registry, so a bad path is not silent (2026-09-25)

Auditing the documented configuration surface against the code found a silent trap: `skills_paths` and
`instruction_files` are read from the user config, `expand_home` resolves `~/…`, and
`tools::skill_roots` *filters out* a configured path that is not a directory — but the only validator
(`config::validate_configured_paths`) is called from `load_user_config_for`, the **project** config loader
that no product entry point uses yet (the known gap recorded in `docs/ACCEPTANCE.md`). So a typo'd path, or a
`~/.agents/skills` that does not exist yet, meant: skills silently absent (`skill` answers "no skills
configured" only when the model happens to ask) and instruction files silently missing from every prompt,
with nothing anywhere telling the user.

`teamagents doctor` now reports both: `[ok  ] skills  1 skill(s) under 1 configured root(s)`,
`[WARN] skills  a configured root does not exist and is ignored, so those skills never load: ~/.agents/skills`,
`[WARN] skills  none configured: skills_paths in the user config registers a root …`, and an
`instruction files` row for the same reason. Verified on a clean first run
(`HOME=/tmp/fresh-home XDG_CONFIG_HOME=… teamagents init && … doctor`): the shipped config registers
`~/.agents/skills`, which does not exist on a fresh machine, and that is now visible as one WARN instead of a
skill list that is quietly empty.

It is a **warning, not a load failure**, deliberately: the shipped `init` config points at the documented
registration root (D-34), and refusing to start a session because a *feature's* root is missing would break
the documented first-run flow (`init → set the key → doctor → teamagents`) on every machine that has not
created it yet. Refusing a bad path stays the behaviour of the project-config loader once that loader is
wired (a decision that needs the user's word).

Evidence: `cli::doctor_reports_the_skills_registry_and_missing_configured_paths` (a root with one skill
reports `1 skill(s) under 1 configured root(s)`; a missing root and a missing instruction file are named;
no configured root says where to put one and adds no instruction-files row) plus the clean first-run probe
above. `make check`, `make pty` and `make verify-model-all` are unaffected and green.

## D-65 A plain reply opens no further turn: the turn storm is over (2026-09-25)

The probe's runaway had a root that is not a policy question after all. The driver's idle rule was
"the last entry is the model's own text **and** no open tasks" — so an instance that still owed a task was
asked again after every reply. A model that answers with prose instead of settling the task (the probe's
worker said `BLOCKED.`) was therefore asked forever: **169 model requests / 1,226,717 prompt tokens / 181
context entries in ~15 minutes**, no progress, bounded only by a goal budget that the session did not have
(D-64 now lets the user configure one, which is a ceiling, not a fix).

Two things say this was a defect rather than a design choice:

- §3 states that a **plain reply settles no task and no goal** — a turn without tool calls is the model
  saying it is done for now. Re-opening a turn against that is the runtime inventing work.
- `V2Control`'s `NoTurnWithoutWork` — *the already-verified model property* — forbids exactly the state the
  clause produced: `MODEL_PENDING`/`TOOLS_PENDING` while the last word is the model's own. The code
  deviated from the verified model, and the model's counterfactual switch (`ReaskAfterReply`) now proves it:
  with the old clause enabled, TLC reports **`Invariant NoTurnWithoutWork is violated`**.

**The fix**: the idle rule counts only *unaddressed* work — content that arrived since the last request (the
boundary drain applied it, D-63) or a task this instance has **not started yet** (`PENDING`). A turn whose
model replied with prose now ends the instance's activity with the task still `RUNNING`. Nothing is invented
about the outcome (§8: the runtime never reads an outcome out of prose) and the loop stops.

**Who resolves it, and why that is the honest ending**: the delegator's `wait` on the task stays pending
(the TUI shows the instance `WAITING` and the task `RUNNING`), and the user has the documented lever: cancel
the task (`c` in the tasks panel, `cancel_task` in the protocol), which **satisfies** the delegator's wait —
a `BLOCKED` task would not, because the wait's task condition accepts `SUCCEEDED|FAILED|CANCELLED` only. So
cancelling wakes the delegator, which can re-delegate or settle honestly; the session is never stuck on a
model that stopped talking, and it never burns a budget doing it. Parking the *task* `BLOCKED` on the
runtime's own initiative was rejected for exactly that reason and because §5.3 reserves `BLOCKED` for a
closed set with no runnable path (the assignee is still runnable — the user can send it work).

Evidence: `v2_supervisor::a_prose_reply_leaves_one_turn_and_the_delegator_resolves_the_task` walks the whole
flow — the worker runs **one** turn and stays `READY`, the task stays `RUNNING`, the leader is `WAITING`, the
user cancels the task, the leader wakes and the goal settles `SUCCEEDED`. With the pre-fix clause restored the
same test fails (`left: Some(4), right: Some(1)`: four requests in 1.5 s). Formally,
`MC_control_reask.cfg` (switch `ReaskAfterReply`) is refuted by `NoTurnWithoutWork` and is part of
`make verify-model-counterexamples`; `make verify-model-all` stays green (7 configurations + the two
two-instance ones).

Ceiling: a session whose model stops settling tasks now *waits* where it used to spin — the delegator's wait
is visible but not self-resolving, so a user who never looks will see a parked turn (and, with `[limits]`
configured, a parked goal). That is the deliberate trade: a visible wait costs nothing, and the alternative
is an unbounded spend. Whether the *runtime* should also offer a bounded "no progress" path (cancel or park
the task after N unsettled turns) remains the user's decision; it is not implemented.

## D-64 The user can bound a goal's cost and time (2026-09-25)

The same audit that produced D-61 and D-63 found the third "designed but unreachable" surface: §8/A18/A35
give a goal a usage ceiling (`limits.max_total_tokens`) and an absolute `deadline`, the control plane really
enforces both (`begin_request` refuses past either and the driver parks the instance with the reason), the
properties are verified (`ReservationsAdmitted`, `AdmissionGate`, `NoRequestAfterDeadline`) — and **no user
surface could set them**. `create_goal`'s limits come from the user config, which carried only `[[checks]]`,
so by default a session ran until the user stopped it. The authority probe's runaway (169 model requests /
1,226,717 prompt tokens, ACCEPTANCE's known gaps) is what made that concrete.

**The surface** is a `[limits]` section in the user config:

```toml
[limits]
max_total_tokens = 2000000   # optional: usage ceiling for every goal this session creates
deadline_minutes = 45        # optional: wall-clock ceiling, counted from goal creation
```

- `max_total_tokens` travels inside `create_goal limits` verbatim, because the core enforces it from there
  (A18). `deadline_minutes` cannot: the core takes an *absolute* timestamp, so `driver::bootstrap` converts
  the duration when it creates the goal and never stores the key on the goal — what is stored is exactly what
  the runtime enforces.
- A zero for either is a config error at load time (it would mean "no request ever"), reported by `doctor`
  and every entry point; like `[[checks]]` this section is **user config only**, never project config.
- `doctor` reports both (`goal limits`), including the honest case "none: a goal (and the session) runs until
  you stop it or the budget is reached".

**Verification.** `config::tests::user_limits_bound_every_goal` (the shape, `{}` when unset, zero refused,
unknown keys rejected), `cli::configured_limits_reach_the_goal_and_really_bound_the_session` (the real daemon:
the goal carries the ceiling, its deadline is ~15 minutes out, the duration key is *not* stored, doctor
reports all three cases) and
`cli::a_tiny_configured_ceiling_parks_the_session_instead_of_running_it` (a 4-token ceiling parks the leader
with `goal … budget exceeded: known 0 + reserved 0 + est 2228 > max 4`). The budget gate itself was already
formal (`V2Control`'s `BudgetFits`/`ReservationsAdmitted`/`AdmissionGate`, A18).

**New formal work**: the deadline was *not* modelled before — `begin_request`'s deadline gate (A35) had only
code tests. `V2Control` now carries `goal.deadlinePassed` (an environment action moves the clock past the
deadline; the fact is monotone), `BeginRequest` requires `~goal.deadlinePassed`, and
`NoRequestAfterDeadline` states the gate. `MC_control_deadline.cfg` is the counterfactual (a runtime that
ignores the deadline, switch `IgnoreDeadline`) and `make verify-model-counterexamples` requires TLC to refute
the property there — it does (`Action property NoRequestAfterDeadline is violated`). The refusal's *park*
shares the classified path `FailRequest` already models.

Evidence: the tests above, `make verify-model-all` (green, `MC.cfg` 6 s / `MC_control_two.cfg` 46 s with the
new invariant and property) and `make verify-model-counterexamples` (six controls, all refuted).

Ceiling: the ceilings are per goal and fixed when the session creates it; amending a *running* session's
limits is still not offered (it is new protocol surface, and the TUI shows the same limits it booted with).

## D-63 An input that arrives during a turn enters at the next boundary (2026-09-25)

The audit of the user-facing surfaces turned up a defect in the *inbound* direction: `submit_input` appended
the input to the instance's context immediately, even while a turn was in flight. A model request is fixed
once it is registered (§3/§6.1), so the input could not be part of it — and the turn's own reply then landed
*after* the input, which left the runtime's idle rule (`step_ready`: the last entry is the model's own text
and no open tasks) satisfied. The message was stored, was never acted on, and the model's answer looked like
the answer to it. The user's message silently did nothing, with no error anywhere — the exact class the design
rules out ("user input enters the target instance at a safe boundary", §5.4).

Reproduced deterministically before the fix (`engine/tests/v2_supervisor.rs`, a scripted provider that holds
its first request open): the reply to the mid-turn input was `{"applied": true}` and the instance made
**one** request where two were owed.

**The fix** keeps the input out of a fixed request: while the instance is `MODEL_PENDING`, `TOOLS_PENDING` or
`COMPLETION_PENDING`, `submit_input` inserts the envelope and returns
`{"applied": false, "queued": true, "phase": …}` (plus an `input_queued` event) instead of appending the
entry. The driver's boundary drains the inbox before it fixes a request (§5.3), so the queued input enters the
conversation in sequence order — after the turn's own answer — and gets a turn of its own. `exec` reports
`input_queued` (and says so on stdout instead of pretending the input landed), and the TUI prints
`queued for <id>: it enters when the running turn ends` so the composer's message is visibly on its way. A
reset seals a queued input with its epoch (A24) and a terminated instance cannot take one; a parked instance
keeps it until it is resumed.

**Verification.** `core/src/v2/control.rs::an_input_inside_a_turn_waits_for_the_boundary` pins the three
paths (READY applies, a turn in flight queues and leaves the phase alone, the drain applies it exactly once
and last); `engine/tests/v2_supervisor.rs::an_input_arriving_during_a_turn_enters_at_the_next_boundary` walks
the real driver: the queued reply, the second turn, and the entry order (the input's index is greater than
the first reply's).

Formally, `V2Control` gained the queue (an instance field, set by `QueueInput` while a turn is in flight, and
cleared by the boundary's `ApplyQueued` / `Input`), a monitor for the landing phase, and two properties:
`InputLandsAtTheBoundary` (an invariant — a user input never lands while a turn is in flight) and
`QueuedInputEntersTheContext` (temporal — a queued input enters the context, unless the instance stops being
active or a reset seals it). `make verify-model-counterexamples` now also runs
`MC_control_midturninput.cfg`, the counterfactual in which the driver applies input mid-turn (the pre-D-63
behaviour): it must refute `InputLandsAtTheBoundary`, and it does
(`Invariant InputLandsAtTheBoundary is violated`).

**A spec correction the two-instance run forced**: `Spec`'s fairness used to be
`WF_vars(\E i \in Instances : Recover(i))`, a *disjunction* over instances. With two instances that lets one
instance be starved forever while the other recovers repeatedly, which is not the system (each instance has
its own driver, and the supervisor drives and restarts them one by one). The new liveness property exposed
it, and the fairness is now per instance: `\A i : WF_vars(Recover(i))`, `\A i : SF_vars(ApplyQueued(i))`
(strong, because a crash loop must not starve the drain) and `\A i : WF_vars(TurnStep(i))` (a turn in flight
eventually ends; `FailRequest` is one of its steps, so an approval wait is covered too). Because the wide
configuration cannot finish in a reasonable time, the fairness check got its own small two-instance
configuration, `MC_control_two.cfg` (same domains as `MC.cfg`, two instances; 1,263,649 states / 165,792
distinct / ~45 s, green, now in `make verify-model-all`), and the older disjunction form is kept as a
counterfactual switch (`PerInstanceFairness = FALSE`) whose configuration
`MC_control_two_disjunction.cfg` **must** refute `QueuedInputEntersTheContext` — it does, in
`make verify-model-counterexamples`.

Ceiling: the queued input waits for the boundary, so a client that sends into a running turn sees its message
acted on only after that turn ends (the TUI and `exec` both say so). Whether the *runtime* should instead
interrupt the turn is the same open question as the re-asking loop (see ACCEPTANCE's known gaps) and is not
decided here.

## D-62 Every request answers every tool call it carries (2026-09-25)

D-61's real-model probe (`review/dogfood/authority.py`) turned up a second defect, in the wire protocol
itself. The worker's first turn ended on an **accepted** `finish`; the runtime answers that call by settling
the turn (or the task) instead of by appending a tool result, so the persisted context keeps an assistant
message whose `tool_calls` are never answered. When that worker was given a new task in the same epoch, its
next request was rejected outright:

```
chat API 400: An assistant message with 'tool_calls' must be followed by tool messages responding to each
'tool_call_id'. (insufficient tool messages following tool_calls message)
```

D-54 fixed the *refused* finish (the runtime now answers it with a receipt); the accepted one, and the
finish that is ignored because it shares a response with other calls (answered by a note, not a tool
message), were still unanswered on the wire — and every later request of that instance failed, which is
exactly the "send a follow-up message" flow.

**The fix** is in the wire projection, not in the stored context: `pair_tool_results`
(`core/src/kernel/instance.rs`) moves an existing answer up next to its call, and now also synthesizes
**one tool-role answer per call the log left unanswered**, naming what it is
(`[no tool result follows: the runtime answered this call outside the tool channel]`). The stored entries are
untouched, no extra model turn is spent, and a call that does have an answer keeps it (the D-54 receipt path
is unchanged).

Evidence: `core/tests/kernel_properties.rs::the_wire_answers_every_call_it_carries` covers the five shapes
(a lone `finish`; two unanswered calls; one answered and one not; a fully answered call — where nothing is
added; an answer that landed behind other entries — moved up), and the exhaustive
`wire_view_is_a_paired_permutation` now asserts that every call in the wire view is answered immediately and
that the only added messages are synthesized answers naming a call the instance really made. Real-model check
(`review/dogfood/authority.py`, second run, isolated state root `/tmp/ta-authority-probe3`): the worker's
first turn ended on an accepted `finish` (entry 18, its task settled `BLOCKED`) and **169 following requests
all imported**, with zero `request_failed` events — the same shape that produced the HTTP 400 in the first run
(which had exactly one, on the worker's first request after the new task).

Ceiling: the synthesized answer also covers a call whose receipt was lost for some other reason — the model
then reads that no result follows instead of the turn dying on a rejected request. The trade-off is stated
in the function's doc comment.

## D-61 The user's authority surface (2026-09-25)

§5.1 makes the user the root of authority, and D-58/D-60 made the *model-visible tool surface* follow the
grants. But no user could exercise that authority: `issue_grant`/`revoke_grant` had no caller outside tests
and the evaluation harness, and the daemon's `grants` view did not even carry the grant's `id` — so no
client could have revoked anything. The visible consequence was that a worker the Leader spawns holds no
`shell@workspace` (§5.1) and nothing in the product could give it one.

**What was added**

- `teamagents authority [list] | grant --subject ID --action A --scope S [--parent G] | revoke --grant ID`,
  a client of the running session's socket (`engine/src/v2/authority.rs`): grants and revocations go through
  `SupervisorHandle::submit_user` like every other business command, so they linearize with driver dispatch
  on the single writer (§9) and the client never opens the database itself. `--json` prints the raw report;
  exit codes are 0 done, 1 the session refused it, 2 usage or no session.
- The guards a human needs and a machine caller does not: the action vocabulary and the pair table live in
  `core/src/v2/capability.rs` (`ACTIONS`, `asks_about`, `authorizes_something`), and the surface **refuses**
  a pair no check asks about (`shell@instance:i-worker` would authorize nothing: never dispatched, never
  offered, never refused) with the scope that action is asked over. A subject that does not exist yet is a
  *warning*, not a refusal — instance ids are chosen by the spawner, so granting ahead of a spawn is
  legitimate and a typo is merely likelier. `revoke` takes the full id or an unambiguous prefix, and `list`
  also prints the session's instances, because a grant's subject is an instance id and there was no other
  headless way to read them.
- The read view gained the fields the surface needs: `daemon.rs`'s `grants` reply now carries `id`, `issuer`,
  `parent_grant_id` and the session's grant revision, and the TUI's topology panel shows the short id next
  to each grant (the TUI already reads the same reply).

**A defect found while wiring it**: the old view read `revoked_at` (a `REAL` column) as an
`Option<String>`, so **one revoked grant made the whole `grants` read fail** with
`Invalid column type Real at index: 1, name: revoked_at`. Reproduced with a standalone rusqlite probe before
the fix. The failure reached the TUI as a lost connection (`DaemonClient::call` clears the connection on any
error), so the grants/topology panel went dead and the client flapped — a documented capability
(`docs/USER-GUIDE.md`'s "revoking a grant removes the tool from the surface") that nobody could have used.

**Verification**: a new TLA+ module, `verification/tla/V2Authority.tla` (+`MC_authority.cfg`, wired into
`make verify-model-all`), models the surface: the user reads the view, writes a grant that the pair table
allows, revokes a grant it can *name*, and the instance's model-visible surface is a **cached** variable that
only its next request refreshes (the code recomputes it per request, `driver::team_kernel`). Properties:
`NoDeadGrantPair` (the surface's table equals the derived table of pairs some check asks about),
`ListedIdsAreUsed`, `EveryLiveGrantBecomesRevocable` (temporal), `AuthorityTracesToTheUser`,
`RevokedStaysRevoked`, `ChildGrantsAreCoveredByTheirParent`, `CascadeTakesTheSubtree`,
`CascadeOnlyTakesTheSubtree` (temporal — the half V2Grants does not state: revoking one grant takes *exactly*
its subtree, never the leader's authority with it), `EffectAtMostOnce`, `OnceStaleNeverExecutes`,
`AuthorizedEffectsOnly` (temporal, with the cached surface), `SurfaceChangesOnlyToTheCurrentEntitlement`,
`StaleSurfaceCatchesUp` (temporal) and `BootstrappedAuthority`. 35,950 distinct states, no error.

Because a property that cannot fail proves nothing, `make verify-model-counterexamples` runs three
configurations that model the plausible mistake and **must** be refuted: `MC_authority_badview.cfg` (the view
without the `id` field — the state of the daemon before this decision) refutes
`EveryLiveGrantBecomesRevocable`; `MC_authority_trustsurface.cfg` (dispatch trusts the cached surface, the
mistake §6.1/A04 forbids) refutes `AuthorizedEffectsOnly`; `MC_authority_stalesurface.cfg` (the surface is
never recomputed) refutes `StaleSurfaceCatchesUp`. The target fails if a control verifies instead.

**Evidence**: `v2::authority::tests::a_revocation_takes_the_full_id_or_an_unambiguous_prefix`;
`cli::the_authority_surface_grants_and_revokes_through_the_daemon` (the real binary against a real daemon:
list carries ids and instances, grant to a spawned worker makes the dispatch question true, dead pairs and
unknown actions are refused, an unknown subject warns, a parent the session does not know is the control
plane's refusal, a derived grant dies with its parent and the dispatch question is false again);
`v2_supervisor::a_users_grant_reaches_the_workers_surface_at_the_next_request` (the granted tool appears on
the worker's next request and a revocation takes it away again — the spec-to-code correspondence of
`StaleSurfaceCatchesUp`); `core/src/v2/capability.rs`'s table test (the pairs the surface accepts are exactly
the ones some check asks); `tui/tests/v2app_tests.rs::frame_shows_the_panels_and_panel_hit_testing` (the
topology line names the id). **Real model** (`review/dogfood/authority.py`, DeepSeek Flash on its native
window, 2026-09-25, isolated state root `/tmp/ta-authority-run`): turn 1 the worker answered *"No — I cannot
run shell commands in the shared workspace. My runtime exposes no bash/exec/terminal tool"* (the §5.1 boundary,
said by the worker itself); the grant went into revision 8; turn 2 the same worker ran the command, reported
exit code 0 and `proof.txt` appeared; the revocation went into revision 11 and left no live shell grant. 13
model requests, both delegated tasks `SUCCEEDED`, no failed request.

**Left open (needs the user's word)**: giving a spawned worker `shell@workspace` *by default* would change
§5.1's spawn contract, and letting the Leader hand out its own shell authority (it holds
`shell@workspace`, and `issue_grant` would accept the narrowing) would turn a boundary the user owns into one
the team manages. Neither is implemented; the surface is the user's.

## D-60 The tool surface follows the grants, and the authority layer is verified (2026-09-25)

The user's rule — a design addition must be formally verified where it can be — had an obvious gap to close
first: the TLA+ set covered the control plane, artifacts, waits, tasks, compression, the daemon and the
required checks, but **authority had no model at all** (no specs mention grants), and D-58/D-59 had just changed
authority semantics: the session's bootstrap grants and the spawn-derived delegate grant.

**The model**: `verification/tla/V2Grants.tla` (+ `MC_grants.cfg`, wired into `make verify-model-all`) models
where a capability comes from — the bootstrap's default grants, narrowing by an instance (manage covers
message/delegate below it, anything else repeats the issuer's own covering grant), the spawn-derived
`delegate@instance:<child>` and its parent link, revocation with the parent-tree cascade and the revision bump,
the dispatch re-check (a live covering grant *and* a still-current stamp), and the offered surface. Properties:
`AuthorizedEffectsOnly` (temporal), `EffectAtMostOnce`, `OnceStaleNeverExecutes`,
`ChildGrantsAreCoveredByTheirParent`, `AuthorityTracesToTheUser`, `RevokedStaysRevoked`,
`CascadeTakesTheSubtree`, `OfferedToolsAreAuthorized`, `BootstrappedAuthority`.

**Result**: `make verify-model-all` is green for all eight modules; the authority model itself is 1,292,517
states / 178,024 distinct / depth 11 / ~1 minute, with TLC's estimated chance that a fingerprint collision hid
a state at 1.2e-9. `make verify-kani` stays green (3 harnesses). Writing the model also falsified its own first
draft twice (an invariant that was really an initial-state fact; a property that ignored that an effect happens
at a *step*, not in a state) — both recorded in `verification/REPORT.md`.

**What the model exposed in the code**: `OfferedToolsAreAuthorized` — the model-visible surface only offers what
the instance's grants back — is **violated by the runtime**: the profile handed `shell` to every instance, so a
child the leader spawned was offered `shell` while holding no `shell@workspace` grant (§5.1's explicit boundary)
and every shell call came back refused. Offering `shell` unconditionally in the model reproduces exactly that
state (TLC: `Invariant OfferedToolsAreAuthorized is violated by the initial state`), so the property is
sensitive to precisely this defect.

The fix keeps the question in one place: `Control::holds_covering_grant(subject, action, resource)` is the
authority question the dispatch re-check already asks, and `driver::team_kernel` now asks it too, dropping
`shell` from the profile unless the instance holds a covering `shell@workspace` grant. The offered tool is
therefore one the instance can dispatch. The spec-to-code correspondence is
`v2_supervisor::the_offered_surface_follows_the_grants`: the leader is offered `shell`/`spawn` (bootstrap), its
spawned child is offered neither, and the test fails when the filter is removed.

User-visible consequence, stated honestly: a spawned worker works with the file/web/skill tools (they need their
binding, not a grant) and has **no shell** until the user grants `shell@workspace` for it. That is the design's
§5.1 boundary; the surface now says so instead of offering a tool that always failed. Opening that boundary by
default (granting shell to a spawned child sharing the project directory) would change the documented spawn
contract, so it stays a question for the user.

## D-59 A spawned team runs: model resolution, spawn-time validation, and a supervisor that survives (2026-09-25)

With D-58 in place the Leader finally *spawned* — and the session stalled instead: two workers sat `READY` with
`PENDING` tasks while the Leader waited, until the headless client's 900-second deadline expired. The daemon log
had the reason: `panicked at src/cli.rs:333: provider for deepseek-flash: model deepseek-flash is not in the user
catalog`.

The chain: the supervisor hands each driver the *wire-effective* profile (its `model` field is the model name the
provider speaks) while the provider factory receives the catalog *key*. For the leader that separation works,
because the supervisor passes the unresolved profile to the factory. A spawned child, though, stores the
parent's **resolved** profile, so its `model` field holds `deepseek-flash` while the factory looks it up as a
catalog key — and the daemon's factory turned that into a `panic!`. The panic happened inside the supervisor's
discovery loop, so the loop died: no further instance was ever given a driver, the workers never ran, and every
delegating Leader waited for work that could not start. The evaluation harness had never seen it because it does
not spawn through the daemon.

Three changes close this, each with its own test:

- `providers::resolve_model` resolves a catalog reference by **key or by the model name the entry declares**, and
  `build_for_model` uses it (its error now lists the available keys). A child's stored profile is therefore
  bootable, which is what the supervisor's own comment already promised for the leader.
- `spawn` gained an optional `model` (a key or a name). The driver resolves it through the catalog *at spawn
  time*, so an unknown reference is that tool call's error — with the available keys in the receipt — instead of
  a provider the supervisor cannot build. This also makes mixed teams reachable: the README has always promised
  that every instance carries its own model, and until now the Leader could not name one.
- The daemon's provider factory no longer panics. An instance whose model cannot be built gets a provider whose
  every attempt fails permanently with the reason (`AnyProvider::Unavailable`), so the instance parks through the
  ordinary classified path (A07: the request closes, the input stays, the instance parks with the reason) and
  every other instance keeps being driven. Ceiling, stated honestly: that parked worker's task stays open, so its
  delegating Leader still waits for it — only the user can cancel the task or terminate the instance (the
  blocked-task flow in AGENTS.md). With spawn-time validation the path is nearly unreachable, and it now fails
  loudly and locally instead of silently and globally.

Evidence: `providers::tests::a_catalog_entry_resolves_by_key_or_by_model_name`;
`cli::an_unbootable_instance_parks_itself_and_the_session_survives` (an instance created with an unknown model,
given work, parks itself with the reason while the leader stays `ACTIVE`); and a real-model delegation run — three instances, both
delegated tasks `SUCCEEDED`, both `[[checks]]` commands passing in the runtime's check round, the whole flow
`instance_spawned → task_delegated → task_started → wait_satisfied → check_round_registered → goal_completed`
in **16.3 seconds** where the same prompt had stalled for 900 (recorded under `review/tmp/dogfood/`).

## D-58 The session authorizes its Leader, so the team feature exists (2026-09-25)

While preparing a multi-instance dogfooding run, reading the grants of a session the CLI had just started showed
exactly one row: `i-leader` / `shell` / `workspace`. Nothing grants `manage`, `delegate` or `message` anywhere in
the product — `issue_grant` had no caller outside tests and the evaluation harness.

That is not a cosmetic gap, because the model-visible tool surface is *derived* from those grants
(`Driver::team_kernel`: `wait` always, then `send`/`delegate`/`spawn` iff the instance holds
`message`/`delegate`/`manage`). So in every real session:

- the Leader was never offered `spawn`, `delegate` or `send` — the model could not even know it might build a
  team, although its instructions tell it to do exactly that, and the README's promise ("you talk to the Leader,
  and the Leader builds the team on the spot") had no mechanism behind it;
- and every collaboration intent would have been refused by `capability_gap` anyway, because the spawner needs
  `manage@session` (the child's `delegate` grant is derived from it).

Why it stayed invisible: the repo's own acceptance evidence came from tests and the A/B/C evaluation harness,
both of which issue these grants by hand before exercising collaboration (`v2_driver`'s spawn tests do it in
their setup). The fixtures therefore passed while the product could not start a single worker.

`bootstrap` (the user's session bootstrap, which already creates the instance and the goal) now also issues
`manage`, `delegate` and `message` over the session for the leader instance — the authority Q5 and D-42 give the
Leader by default. The grants are issued with fixed command ids and *outside* the "instance already exists"
guard, so the call is idempotent (the command-replay path returns the stored receipt) and a session created by
an earlier build repairs itself on the next start instead of keeping its Leader powerless.

Evidence: `cli::the_daemon_grants_the_leader_the_team_authority` reads the grant table through the daemon's own
protocol after starting it the way a user does; `v2_driver::the_leader_is_authorized_to_build_the_team_by_default`
asserts the *first request's* tool list contains `spawn`/`delegate`/`send`/`wait`, that a scripted `spawn` with a
task really produces `instance_spawned` (`task: true`) without any hand-issued grant, and that the three grants
are session-scoped. Both fail against the previous code (the tool list came back without `spawn`).

## D-57 `--cwd` reaches the session, so the agent works where it was asked (2026-09-25)

The first dogfooding run — this repository's own `edit-integrity` evaluation fixture, copied to a scratch
directory, with the fixture's `checks.txt` configured as a `[[checks]]` entry — reported `SUCCEEDED` and the
artifact verified independently, but it took **60 model turns and 291 seconds** for a one-line INI edit, and
the transcript showed why: almost every turn was spent exploring the *TeamAgents repository* (`review/eval/**`,
`docs/DECISIONS.md`, `engine/src/v2/driver.rs`, the probe's own `session.sqlite`, even `ps` for the harness
processes). It only found the intended file by inferring the probe layout and wrote it by absolute path. Its
workspace was the repository, not the `--cwd` it had been given.

Root cause: `ensure_daemon` passed `--state-root`, `--model` and (since D-55) `--full-auto` to the daemon it
starts, but never `--cwd`, so the daemon — and therefore every instance and tool in the session — inherited
the *client's* process directory. `teamagents --cwd DIR` and `teamagents exec --cwd DIR` silently worked
elsewhere; only `teamagents daemon --cwd DIR` ever honoured the flag. The D-49 `--check` commands were correct
because that workspace is resolved client-side, which is exactly why the acceptance gate passed while the
agent worked in the wrong tree.

- `ensure_daemon` now takes a `DaemonRequest` (state root, model, `full_auto`, `cwd`) and forwards `--cwd`.
- The greeting carries `workspace` next to `permissions`, and the client's note reports any requested setting
  that could not apply to a session that is already running, naming the session's real workspace.
- `exec --json` reports `session_workspace` next to `workspace`: the former is where the session works, the
  latter where its own `--check` commands run, and the two differ when the client joins a live session.
- The greeting's fields live in one `SessionFacts` struct threaded through the accept loop, instead of a
  parameter list that had already grown once (D-55).

The pre-fix run also left the edited fixture (`app.ini` with `retries = 5`) in the **repository root**: the
file it was asked to edit, written into the tree the session had actually been given. (The stray file was
removed once recorded.)

Evidence: `cli::cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one` — the daemon reports the
requested directory, the instance's `workspace_ref` (the root the tools use) is exactly that directory, and a
second client with another `--cwd` is told the live one; without the forwarding the test fails showing the
daemon on the client's own directory. The dogfooding run was repeated after the fix with the same fixture and
configuration: **6 turns and 9.1 s** (from 60 and 291 s), instance workspace correct, artifact verified, and a
second fixture (`rust-fix`, `cargo test` as the check) ran in 8 turns and 8.4 s with its acceptance command
passing both in the runtime's check round and independently (evidence under `review/tmp/dogfood/`).

## D-56 Kernel protocol notes reach the model (2026-09-25)

`interpret_response` produces protocol notes when a response mixes calls that exclude each other — a `finish`
that is not the only call, or a combined `wait` — and `import_interpretation`'s own comment said they "join the
context as their own entry in a follow-up input". They did not: the driver only printed them to the daemon's
stderr, so a model whose completion was ignored never learned why and could repeat the same mistake every turn
(the same channel gap that made a refused `finish` unrecoverable before D-54).

`import_response` now accepts the notes and appends them as one `note` entry (user-role text prefixed
`[protocol note]`, deduplicated by `note-<decision_id>`, atomic with the import so a crash cannot lose them),
and the driver passes `interpretation.notes` with the import. A refused `finish` deliberately carries no note
any more: it rides as an ordinary intent, so the runtime answers that call with the same problem and a note
would say it twice.

The ordering is what makes this safe: `materialize`'s `pair_tool_results` moves every answer next to the call it
answers, so a note written before the decision's receipts still reaches the model *after* them — the order
strict wire endpoints require (D-54).

Evidence: `v2::control::tests::protocol_notes_reach_the_model_after_the_receipts` (the note entry, its dedup
envelope, and the wire order assistant → tool → note built by the real kernel),
`kernel::tests::a_finish_without_a_usable_status_is_not_a_completion` (a refused finish carries no note),
`v2_driver::an_ignored_finish_reaches_the_model_as_a_note` (the model's *next request* contains the note; it
fails without the plumbing).

## D-55 `--full-auto` reaches the session it starts, and the mode is visible (2026-09-25)

A probe run parked on an approval although it had been started with `--full-auto`: both entry points parsed
the flag and dropped it. `ensure_daemon` took only a model key, so `teamagents --full-auto` and
`teamagents exec --full-auto` started a plain `approved_scope` daemon — the documented flag (README argument
table, user guide §4) was a no-op, and the user guide's troubleshooting advice ("or run with `--full-auto`")
did not work either. Only `teamagents daemon --full-auto` ever took effect.

- `ensure_daemon` takes the flag and forwards `--full-auto` to the daemon it starts, and reports whether it
  had to start one.
- The daemon greeting now carries `permissions` (the mode the session booted with). It is additive: clients
  that do not know the field ignore it, and the mode is fixed for the session's lifetime (D-41), so the
  greeting is the honest place to state it.
- Calling `exec`/`teamagents` with `--full-auto` against a session that is already running prints
  `note: a session is already running for this state root in <mode> mode, so --full-auto did not apply …`
  instead of silently pretending. `exec`'s JSON report carries `permissions` as well, so a CI log records
  which mode the run happened in.

Evidence: `cli::full_auto_reaches_a_started_daemon_and_is_reported_against_a_live_one` starts a session
through `exec --full-auto`, asserts the daemon greeting reports `full_auto`, then runs against that live
session and asserts the note names the real mode (and that the daemon it started is stopped again).

## D-54 The completion path survives a real model (2026-09-25)

Several things fixed earlier were verified again against a real DeepSeek Flash session (isolated state root, native
1,000,000 context window per D-36, effort `high`). Two of those runs failed in ways the scripted providers
cannot express, because they emulate neither a strict wire endpoint nor a model that forgets a field:

- **The repair turn after a failing check was wire-invalid.** The model's `finish` call is an assistant
  `tool_calls` entry with no operation of its own; on the check path nothing answered it, so the transcript
  sent to the repair turn contained an assistant message whose call had no tool answer following it. DeepSeek
  answered `HTTP 400 … An assistant message with 'tool_calls' must be followed by tool messages responding to
  each 'tool_call_id'` and the run died as a permanent model error. `register_check_runs` now answers the
  dangling call (with `[finish received: required checks round N must pass before the goal can settle]`)
  before it appends the synthetic check entry, so the transcript stays valid on both the repair and the
  blocked path.
- **A `finish` without a usable status produced a false failure.** The kernel mapped a missing or unknown
  `status` to `failed`, the goal closed as `FAILED` although the work was done, and the required checks never
  ran (they only run for a claimed success). The kernel now refuses such a call: it produces no completion,
  reports the problem, and hands the call to the runtime as an ordinary intent, which answers it with
  `finish refused: status is required and must be one of success, blocked, failed …` and continues the same
  turn — the uniform "invalid call → error receipt → model retries" path. A stated outcome (`success`,
  `blocked`, `failed`) is unchanged, and the goal can no longer be settled by a call that states nothing.

The structural rule both fixes protect is now asserted where the scripted providers cannot fake it:
`v2_driver::assert_wire_valid` walks the real transcript and requires every assistant `tool_calls` entry to be
answered before the next assistant message (this is what strict endpoints check), and
`a_finish_without_a_status_is_corrected_in_the_same_turn` requires the correction, the continued turn and the
required check. Both tests fail against the pre-fix code (`assert_wire_valid` reported
`an assistant message followed unanswered tool_calls: ["finish-…"]`; the status test reported the goal as
`FAILED`), which is how they were checked.

Evidence: `core::kernel::tests::a_finish_without_a_usable_status_is_not_a_completion`,
`v2_driver::the_check_repair_path_keeps_the_transcript_wire_valid`,
`v2_driver::a_finish_without_a_status_is_corrected_in_the_same_turn`, plus five real runs recorded under
`review/tmp/d50-live/` (one positive gate run, one 400-error run, one false-`FAILED` run, one approval exit-3
run, and the post-fix re-runs). The scripted tests in this repository accept any transcript, so this is exactly
the class of defect the real-service rule exists for.

## D-53 Documentation claims corrected to the code (2026-09-25)

Continuing the documented-surface audit (D-49 … D-52), three prose claims did not match the implementation. All
three are now written as the code behaves, in both READMEs and in `AGENTS.md`:

- **"an approval … `once` expires after use"** implied an approval *mode* that no longer exists. There is one
  approval action, and the dispatch re-check requires the approval row to be `APPROVED` with exactly the
  operation's `args_hash` and `grant_revision` (plus an optional `expires_at`), so an approval covers one
  dispatch of one operation. The text now says that. (The neighbouring claim — a pending approval of an
  operation that settles, or of a terminating instance, expires automatically — is real:
  `expire_pending_approvals` is called on those transitions.)
- **"file read/write/search with atomic multi-file edits"** overstated `edit_file`, which replaces exactly one
  occurrence in one file (`old_string` must be unique, optional `expected_sha256`); a process-wide write lock
  serializes mutations and writes are atomic per file. The text now describes that instead of promising one
  atomic multi-file edit.
- **"`Enter` send, `Shift+Enter`/`Ctrl+J` newline"** in the key list was aspirational until D-52 made it true,
  so the wording there was fixed together with the implementation (and now names `Ctrl+J` as the chord that
  works in every terminal).

## D-52 The composer really is multi-line (2026-09-25)

Both READMEs (and `tui/src/text.rs`'s own module comment) promised "`Enter` send, `Shift+Enter`/`Ctrl+J`
newline", and `Composer::insert_newline` existed — but nothing called it for *typed* keys: `Enter` submitted
unconditionally, so `Shift+Enter` sent the message, and `Ctrl+J` was swallowed by the "control chords never
insert text" rule. Only pasted text could carry a newline. A coding agent's main input is often a multi-line
instruction, so this was a real gap between the documented interface and the code.

`composer_key` now inserts a newline for `Ctrl+J` and for `Shift+Enter` before the submit arm. `Ctrl+J` is the
universal one: in raw mode the terminal's `0x0A` reaches crossterm's parser as `Char('j') + CONTROL`
(crossterm documents exactly that for `\n`, which is why the chord works in any terminal), while
`Shift+Enter` needs a terminal that reports modifiers — the TUI already pushes the keyboard-enhancement flags,
so modern terminals do. The footer hint now advertises `Ctrl+J newline`, reordered so that a narrow terminal
truncates the least important chord instead of the pending-approvals hint (78 characters with one approval
pending, so it fits an 80-column terminal as well).

Evidence: `v2app_tests::composer_inserts_newlines_and_submits_the_whole_text` (both chords, and a plain
`Enter` still submits the whole text) and — in a real terminal, through the real binary —
`make pty`'s `pty_v2_smoke.py`, which types two lines with `Ctrl+J` in between and asserts that the submitted
input is exactly `"first-line\nsecond-line"`.

## D-51 The coordinator lock absorbs the fork/exec window (2026-09-25)

`make check` failed intermittently (roughly every second run on a loaded machine) with
`state root already has a coordinator: … the operation would block`, always in a test that restarts a
coordinator after a simulated crash (`v2_driver`'s two crash tests, `v2_mcp`'s recovered-dispatch test). The
failure reproduced on unmodified `HEAD` (a pristine `git archive` copy failed the same way), so it was not a
regression from the work in D-49/D-50 — it was a real hazard the tests happened to trip.

**Root cause.** `jobs::state_lock` holds the coordinator lock with `flock` on a `File` opened `CLOEXEC`, which
is what the design means by "never inherited by tool children" (§6.1). But `CLOEXEC` only takes effect at
`exec`: `Command::spawn` (a shell-job runner, an MCP server, a hook) forks a child that, until it execs,
inherits the *entire* descriptor table of the process — including every coordinator lock any thread is
holding. During that window the lock is genuinely alive, so a `try_lock` from another thread or process gets
`EWOULDBLOCK` and the restart reports a coordinator that is about to disappear. Under load the window widens
(the child is not scheduled promptly), which is exactly when the failures clustered; with the whole suite
serialised or the tests run alone, the window never overlapped the restart.

The probe evidence: on a failing run the lock file had exactly one descriptor in the panicking process (the
fresh one) while a *different* process whose `cmdline` was still the test binary held an inherited descriptor
to the same file — a pre-`exec` child. A 25-round sequential `start → crash → start` loop on one state root
never failed, which ruled out a leaked handle; only the concurrent-spawn case did.

**Fix.** `jobs::state_lock` now waits up to two seconds (10 ms steps) for a busy lock instead of failing
immediately, and still fails with the same message once the wait is over. Exclusivity is unchanged — the
kernel grants the lock to exactly one holder, and a genuine second coordinator keeps it for far longer than
the window, so A33's "a second daemon is refused" still holds (it just takes the bounded wait to say so).
A filesystem that cannot lock at all (any error other than `WouldBlock`) still fails immediately, because
that is a hard error rather than a busy lock.

While chasing it, a second, unrelated flake surfaced once in a five-run loop: `budget_exhaustion_parks_the_instance`
and `goal_deadline_parks_the_instance` read the lifecycle once, immediately after the refusal event. The park is
a separate committed step from the refusal, so under load the snapshot could still show `ACTIVE`. Both tests now
wait for the lifecycle they assert on (`wait_lifecycle`, alongside the existing `wait_phase`), which is what they
meant in the first place; the product behaviour was correct.

Evidence: `jobs::tests::a_momentarily_held_lock_is_absorbed` (a short hold is absorbed and the winner waited),
`a_live_holder_is_refused_after_the_wait` (a live holder still loses, with the A33 message),
`the_default_wait_is_bounded`; `v2_driver::a_crashed_driver_releases_its_coordinator_lock` covers the release
contract itself. After both fixes, `make check` ran green five times in a row under exactly the conditions that
failed in roughly half of the earlier runs (four runs before the second fix: no lock failure remained, one
lifecycle-race failure).

## D-50 User-defined completion checks become reachable (2026-09-25)

The design requires that "required checks defined by the user or project must pass" (§8, Q11) and the runtime
implements the whole path — `create_goal limits.required_checks` → the driver registers the check operations
at the completion boundary → a failing check enters repair and finally blocks the goal (A16/A17). What was
missing was any way for a *user* to define them: the goal limits were hardcoded to `{}` in `cli::daemon`, so
in practice every real session settled on the model's own report. D-49 recorded that as the ceiling; this item
closes it with the smallest surface that fits the existing conventions.

- **`[[checks]]` in the user config** (`id`, `command`, optional `timeout`, `network`, `inputs`) is now the
  user's acceptance contract. `config::goal_limits` turns it into the goal limits the daemon boots with, and
  the *same* core validator that guards `create_goal` runs at config load — so the config edge and the control
  plane cannot disagree, and a broken entry (empty command, `timeout = 0`, an escaping input) fails `doctor`
  and every entry point instead of a goal silently never settling.
- **User config only**: `checks` joins `hooks` and `retention` in the list of sections a project file may not
  define. A check runs unattended at the completion boundary without an approval prompt, so letting a cloned
  repository install one would be remote code execution by config. (Wiring the project config into the daemon
  at all is a separate, still-open question; today only the loader knows about it.)
- `doctor` reports how many checks will gate the session and which ids they are, and `[[checks]]` is
  documented in `examples/config.minimal.toml`, `examples/config.toml` and the user guide (§2.1).
- The TUI names the checks of a round and the ids that failed (`v2app::apply_events`), so a goal being
  repaired or parked explains itself while the check receipts show up in the conversation as tool results.
- The headless client's `--check` (D-49) keeps its v1 semantics and is documented as the *weaker*, client-side
  acceptance command: it decides `exec`'s exit code after the turn, while a runtime check prevents the goal
  from settling at all.

Evidence: `config::tests::user_checks_become_goal_limits` (the exact JSON shape; no `null` field may reach
`create_goal`) and `a_broken_check_is_rejected_when_the_config_loads`; the user-only rule in
`config::tests::user_hooks_and_retention_survive_loading_and_project_ones_are_ignored`;
`v2_driver::configured_checks_gate_the_goal_through_the_config_edge` drives config text → goal limits →
a failing check → repair → `SUCCEEDED`; `cli::the_daemon_carries_configured_checks_into_the_goal` proves the
running daemon stores them on the goal and that `doctor` reports them;
`tui::events_drive_refreshes_and_notes` checks that a round names its checks and a repair names the failing one.

## D-49 The headless `exec` contract is real again (2026-09-25)

While comparing the product surface with the code, the headless entry point turned out to be documented but
partly not implemented. `teamagents exec "prompt"` (the plain form in both READMEs) was rejected as a usage
error because the argument parser demanded `--json`; `exec -` accepted stdin according to the README but
submitted the literal string `-`; `--check COMMAND` was parsed and then silently dropped. Three further
defects came out of the same review: a settlement recorded by an **earlier** run was replayed as the current
run's outcome (the client started at watermark 0), a goal that settled `FAILED`/`BLOCKED` still exited `0`,
and a leader parked by a permanent failure or a click-through approval let `exec` wait for its whole
deadline.

The restored contract is the v1/D-32 one, which is what both READMEs already promised:

- **Prompt**: the positional argument, or everything on stdin when it is `-`; an empty prompt is a usage
  error. Both READMEs document the marker.
- **Exit codes**: `0` settled (goal `SUCCEEDED`, or a direct reply), `1` failed or unfinished (including a
  failed `--check`), `3` an approval is pending (a headless run has nobody to answer it, so it reports
  immediately instead of burning the deadline), `124` the deadline passed, `2` usage or infrastructure.
- **`--check COMMAND`** (repeatable): the user's own acceptance command, run after the turn ends, in order,
  in the isolated shell inside the client's workspace; the first failure stops the list and makes the run
  fail. The verdicts are printed, written to `<state root>/verification.json` and carried in the `--json`
  report. The client reads each command's status from a random marker the wrapper prints, so a command that
  prints its own `(exit 0)` cannot fake a pass. It never runs when the run stopped for an approval.
- **Own outcome only**: the client drains the event log before submitting (a stored `goal_completed` is
  history, not this run's result), reports a permanently failed leader request as the run's failure at once,
  and refuses to submit to a leader whose lifecycle is not `ACTIVE` (parked/paused) instead of queueing work
  nobody drains.
- **Daemon startup**: the daemon's output goes to `<state root>/daemon.log`; when it exits during startup the
  caller reports its own words (and that path) in under a second instead of waiting the full 30-second
  socket window.

**Left open at the time**: `--check` is a *client-side* acceptance command, so it does not become the goal's
runtime `required_checks` (`limits.required_checks`, executed by the driver at the completion boundary,
D-42/§8), and no user surface predefined those. D-50 closes that gap with `[[checks]]` in the user config and
keeps the two clearly distinguished; amending the goal limits of an *already running* session is still not
offered (it would be new protocol surface, and it has no user request behind it).

Evidence: `v2::exec::tests::exit_codes_follow_the_documented_contract`,
`the_check_verdict_reads_the_wrapper_marker`,
`acceptance_commands_run_in_order_and_stop_at_the_first_failure`; `main::tests::exec_takes_the_prompt_from_the_argument_or_from_stdin`,
`exec_refuses_a_missing_or_empty_prompt`; and against a real socket with a scripted leader:
`v2_daemon::headless_runs_report_their_own_outcome_not_an_earlier_settlement`,
`headless_runs_verify_the_acceptance_commands_and_gate_the_exit_code`,
`a_failing_acceptance_command_fails_the_run`, `a_blocked_goal_is_not_reported_as_a_success`,
`a_failed_turn_ends_the_headless_run_instead_of_timing_out`,
`a_parked_approval_ends_the_headless_run_at_once`; and through the real binary:
`cli::exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check`.

## D-48 TUI shortcuts without function keys (2026-09-25)

The user pointed out that some keyboards have no function keys, so the interface no longer binds any. View
switching is now:

| Key | Effect |
|---|---|
| `Ctrl+N` | cycle the views: conversation → instances → tasks → topology → conversation (works while composing) |
| `Ctrl+A` | jump into the approvals box (only when approvals are pending) |
| `Esc` | back to the conversation from a panel (unchanged) |
| `Tab` | switch the conversation target (unchanged) |
| `Ctrl+C` / `Ctrl+D` | quit (unchanged) |

The removed bindings were `F1` (conversation), `F2` (approvals), `F3` (instances), `F4` (tasks) and `F5`
(topology). Panel-local keys (`Enter`, `p`, `r`, `t`, `c`, arrows) are unchanged, and the footer hint line now
advertises `Ctrl+N` / `Ctrl+A` instead of the function keys. Control chords never insert text into the
composer, so a `Ctrl+<letter>` press can no longer leave a stray character behind.

Evidence: `tui/tests/v2app_tests.rs::view_switching_cycles_with_ctrl_n_and_esc_returns` walks the cycle,
asserts that pressing `F3` leaves the view unchanged and that the hint mentions `Ctrl+N`;
`instances_panel_pauses_resumes_and_switches_the_conversation`, `tasks_panel_cancels_only_live_tasks`,
`termination_requires_an_explicit_confirmation` and `the_palette_drives_panels_selection_and_status` reach
their panels through the cycle. `make pty` drives the real terminal with the `Ctrl+N` bytes, and `make check`
is green.

## D-47 TUI colour scheme: the v1 palette (2026-09-25)

The user preferred the v1 TUI's colours over the ones the current conversation UI used. The palette is restored
as `tui/src/theme.rs` (the Codex palette: `BG` 0x0d0d0d, `PANEL_BG` 0x181818, white foreground, `GREY` 0x5d5d5d,
`ACCENT` 0x3b82f6, green success, `NOTICE` 0xafafaf, red error, yellow warning plus `SELECT_BG`/`ZEBRA_BG`/
`HOVER_BG`) and applied to the current UI:

- the whole screen sits on `BG`;
- every bordered panel uses an `ACCENT` border, a `PANEL_BG` surface and an accent title;
- the selected list row is the accent surface with panel-dark text (v1's selection look);
- the status line sits on `PANEL_BG`, and turns dark-on-red while disconnected;
- chat labels follow the v1 convention: grey bold labels, white body text, accent for the assistant, notice
  grey for machine-generated text and red for errors; the footer is grey.

The v1 TUI itself (its layout, tabs, forms and slash commands) is **not** restored: it drove the retired
backend. Only the colour scheme was ported, which is what was asked; further v1 interface elements can be
ported one by one on request (the source stays in Git history, `git log -- tui/src/ui.rs`).

Evidence: `tui/tests/v2app_tests.rs::the_palette_drives_panels_selection_and_status` asserts the screen
background, the accent panel border, the accent-surface selection and the panel-surface status line on a
TestBackend frame; `make check` and `make pty` are green.

## D-46 Workspace policies wired into spawn (2026-09-25)

D-45 found that the shared/isolated/git-worktree policies in `engine/src/workspace.rs` ([design](DESIGN.md)
§12.3, Q14) were only called by their own unit tests. After the user confirmed "wire it up":

- **Model-visible entry**: the `spawn` tool gains an optional `workspace` argument — `shared` (default: the
  project directory), `isolated` (a private directory at `<state root>/instances/<id>/work`) or
  `git_worktree` (the instance's own branch and worktree). An unknown value fails that tool call (a
  `collaboration`-class receipt) and **never** kills the driver.
- **Resolution and record**: `driver::prepare_spawn_workspace` resolves the policy before the instance
  starts, creates the directory or worktree and writes the result to `<instances_dir>/<id>/workspace.json`
  (atomic replace). `workspace_ref` points at the resolved directory and the tool receipt carries
  `path/policy/note`, so the model can see whether it got a shared or isolated workspace and why a fallback
  happened.
- **Fallback**: asking for a worktree in a project that is not a Git repository or has uncommitted changes
  runs in shared mode and says why in `note` — uncommitted input is never ignored silently.
- **Retirement**: when an instance reaches `TERMINATED` the supervisor retires the workspace from its record.
  A shared record only drops the record; an isolated directory that holds anything besides our own
  `INPUTS.md`, or a worktree with uncommitted or unmerged work, is **refused and reported** (the site is
  kept). Everything else is removed together with the record, and a failed retirement never affects
  termination itself.
- **No garbage on failure**: when the control plane refuses a spawn, the prepared directory is kept (nothing
  is deleted on a guess) and a retry with the same instance id reuses it.
- Evidence: `engine/src/workspace.rs` unit tests (policy, fallback, record, retirement, including "uncommitted
  work is not deleted"), `engine/tests/v2_driver.rs::spawn_resolves_the_requested_workspace_policy` (isolated
  and worktree rows plus receipts; an unknown policy is refused without creating a directory) and
  `engine/tests/v2_supervisor.rs::terminating_an_instance_retires_its_workspace` (the directory and its record
  are retired, the shared project stays). Docs: `docs/USER-GUIDE.md` §3 and the README feature list.
- Side cleanup: `AgentSpec`/`RuntimeKind` in `core/src/models.rs` were only used by the old signature and
  went away with the change to `prepare(id, policy, project_cwd, member_dir)`.

## D-45 Cleaning earlier-implementation leftovers and restoring `[hooks]` (2026-09-25)

The user confirmed, item by item: (1) rewrite history; (2) remove the earlier implementation's code;
(3) drop doctor's Codex probe; (4) delete the two `#[ignore]`d old entry-point tests; (5) restore `[hooks]`
with the existing wire protocol; (6) keep `review/tmp/` as the probe area; (7) push.

- **hooks (5)**: `engine/src/hooks.rs` keeps the existing wire protocol — for `notify`, argv[1] is the event
  name and the event JSON arrives on stdin; it is asynchronous, bounded at 10 seconds and only logs failures
  to stderr. `pre_tool` runs synchronously before every native tool call: exit 0 allows, exit 2 denies (the
  first stderr line becomes the reason handed to the model) and any other exit code, spawn failure or timeout
  allows the call while logging to stderr. The event set is `tool_call` (with
  `tool`/`arguments`/`ok`/`error`), `team_action`, `run_completed`, `run_failed`, `run_cancelled` and
  `run_paused` (the edge into PAUSED; the value seen at boot does not count). A replay after crash recovery
  is not asked again (the decision was made at first dispatch), and required checks are the user's own
  acceptance commands rather than model tool calls, so they skip `pre_tool`. Evidence: the `hooks.rs` unit
  tests plus `a_pre_tool_hook_vetoes_a_tool_call_and_the_turn_continues` and
  `notify_hooks_receive_tool_call_and_run_completed` in `engine/tests/v2_driver.rs`; doctor still checks that
  hook programs are executable.
- **Old control plane removed (2)**: deleted `core/src/{control,storage,views,server,references}.rs`, the
  `teamagents-core` stdio binary, the six test files that existed for it and the `core/src/models.rs` types
  only those used; `BUILTIN_TOOL_BINDINGS` moved to its single product use site, `engine/src/bound.rs`. Core
  dropped from 243 to 91 test cases, keeping every case the product path and the acceptance matrix cite
  (`core/src/v2/control.rs` unit tests, `v2_invariants`, `kernel_properties`).
- **Doctor's Codex probe (3)**: the current implementation has no Codex member type, so the
  `codex app-server` and `codex protocol schema` checks and their helpers were removed. The
  "no longer supported" errors for `--resume/--team/--plain` stay, because a clear message beats silently
  ignoring the argument.
- **Two old entry-point tests (4)**: they could only fail if enabled (they assert the removed entry points
  return `ok:`), so they went away with the code.
- **History rewrite (1)**: `verification/tla/states/` (TLC state files, roughly 23 GB uncompressed) appeared in
  two unpushed commits only and was removed with
  `git filter-repo --path verification/tla/states --invert-paths`. Verification: `HEAD^{tree}` and
  `git ls-files` are identical before and after (content unchanged), the path has no objects left in history,
  and the pack dropped from 6.18 GiB to about 27 MiB. The pre-rewrite `.git` backup was kept next to the
  repository and deleted once the result was confirmed. The commit id cited in `verification/REPORT.md`
  (`bc536bb5`) was updated to the rewritten `d37e1b4`.
- **Workspace policies**: untouched here; the user then chose "wire it up", see D-46.

## D-44 Two fixes found by formal verification (2026-09-24)

Landed after the user confirmed "fix everything". Both findings started as counterexamples from the TLA+/TLC
specs and were confirmed with code probes; each has a regression test. The fix ledger is in
[review/fix-notes-verification-2026-09-24.md](../review/fix-notes-verification-2026-09-24.md).

- **V-W1: a wait's tool_call was not answered.** Only the drain path answered it; the "satisfied at
  registration" and "superseded/closed epoch" paths moved the wait to SATISFIED/CANCELLED without appending
  a tool response, so a strict wire endpoint rejects the next request. Fix: `answer_closed_waits` extends the
  answer to both paths (same reason text as the drain, deduplication key stays the wait id). Spec side:
  `ResolvedWaitIsAnswered`; regression `wait_call_answered_outside_the_drain_path`.
- **V-G1: a settled goal still accepted new work.** Delegation, opening operations and continued billing all
  ignored the goal status, so new turns kept billing a settled goal. Fix: `budget_goal` accepts only ACTIVE
  goals (both the instance pointer and the oldest open task path), `complete_goal`/`block_goal` detach the
  instance pointer on settlement (`detach_goal`, with `detached` in the response and events), and
  `delegate_task` requires an ACTIVE goal and tells the caller to create one first. Spec side:
  `NoStaleActiveGoal`, `RegisteredWorkNeedsAnActiveGoal`, `RequestsResolveToActiveGoals`; regression
  `a_settled_goal_takes_no_new_work`.
- **Deliberate semantic boundary**: the linearization point for "new work" is the request, not the operation.
  A request admitted while the goal was ACTIVE may still open operations and bill that goal after settlement —
  that is honest accounting, not new work. `complete_goal` checks open operations but not tasks, so a goal can
  settle while its own tasks are still open and those tasks' later requests have no billing goal; tightening
  that would need a committed "completion refused" result for the driver and is out of scope here.
- **V-P1: stale execution pointer after termination.** The spec-to-code correspondence test
  (`core/tests/v2_invariants.rs`) found, during a random walk, an instance whose phase stayed
  `MODEL_PENDING` with `active_request_id` pointing at a cancelled request. Fix: the termination branch
  normalizes the execution pointer exactly like `reset_instance`/`fail_request`. Regression
  `terminating_an_instance_normalizes_its_execution_pointer`.
- **V-P2: a compression request could be imported as a turn.** The same correspondence test reached
  `import_response` on a compression request and it succeeded. Fix: the control plane refuses imports whose
  `kind != 'turn'` (compression is submitted by `compress_context`); regression
  `import_response_refuses_a_compression_request`.
- Evidence: `make verify-model-all` (control plane, artifacts, waits, tasks and compression all exhaustively
  green) and `make check` (including `core/tests/v2_invariants.rs`: all command sequences up to length 2, 60
  fixed-seed walks, coverage assertions and a negative control for checker sensitivity). The property ↔ code
  ↔ acceptance-item mapping is in [verification/README.md](../verification/README.md).

## D-43 Provider edges aligned with the pi coding agent (2026-09-24)

The user asked for multi-provider support to follow the pi coding agent directly (the `pi-ai` package in the
`earendil-works/pi` repository). The gaps from the earlier comparison were landed by current impact:

- **Landed**: tool-call ids normalized across protocols (the same id maps consistently within one request) and
  `max_tokens` clamped to the remaining context (4096 safety margin) — both had already landed earlier. This
  round added provider-independent retry-text classification (a 429 carrying quota/billing-exhausted wording
  becomes Permanent before the status-code table) and effort normalization at the config edge (deepseek
  xhigh→max keeps the existing user decision; anthropic xhigh/max→high follows pi's `clampReasoning`; anything
  else passes through, since a catalog entry is the user's declaration of what the model supports).
- **Not landed yet**: image downgrading (the runtime has no image flow; a `ponytail:` comment records the pi
  style upgrade path — declare input modalities in the catalog plus a placeholder at the edge), a cost rate
  catalog (A18 bills tokens, not dollars yet) and more protocol adapters such as google/vertex/bedrock (add
  them when one is needed; the adapter pattern is in place).

## D-42 System direction and scope confirmed (2026-09-23)

After 19 requirement clarifications and a re-examination of the Python kernel and LangGraph, the user
explicitly chose: "a small Rust kernel + a persistent Rust instance runtime + SQLite + separate tool process
management + a Rust TUI", and asked for every rationale and alternative to be re-examined. This record
confirms the system direction and scope.

- The product stays in Rust. The small kernel owns concise model interaction and execution decisions, and the
  persistent instance runtime owns long-horizon tasks, permissions, scheduling, recovery and tool execution;
  Python/LangGraph are not a premise.
- A team of one is legal; instances isolate context, messages and tool access by default. The Leader manages
  by default and can delegate a limited subset; authorized instances may talk directly and the connection
  graph may be arbitrary, replacing D-33's member-to-member restriction. The shared project directory is
  granted by default, with isolated directories or worktrees on demand; `full_auto` is still only logical
  isolation.
- The first usable version includes the Rust TUI, basic file/shell/web tools, MCP, Skills, multiple providers
  and mixed models inside one team. No external Codex backend is needed; short-lived helpers use the same
  instance mechanism, and D-39's separate helper loop is no longer an architectural requirement.
- Background work survives CLI/TUI exits; the user can reconnect, read any instance's history, talk directly
  to an instance and pause or cancel it. Those user permissions are not automatically granted to the Leader or
  other agents. Instances are reused within a session, can be terminated or reset, and are isolated across
  sessions by default.
- Work continues by default with an optional goal-level budget, and permanent failures or repeated failures
  are handled in a bounded way. A restart resumes authorized work; when an external outcome is unknown it is
  verified first, and if it still cannot be confirmed the affected tasks park and notify instead of being
  replayed blindly.
- Required checks must pass and every other completion claim carries evidence and unverified items;
  independent review happens on demand and never forces extra instances. Acceptance weighs success rate and
  long-horizon reliability at the same model and budget: single-instance behaviour must not regress and
  on-demand collaboration must show a reproducible gain. Costs are estimated on a small scale before the
  formal evaluation budget is set; this item contains no real-model calls.
- DeepSeek Flash is the user-confirmed DeepSeek V4.1 Flash and the main acceptance baseline with its native
  1,000,000 context. D-36's native-window rule and D-41's `full_auto`/`approved_scope`, process-group
  cancellation and credential-environment semantics stay in force.
- No compatibility with old configs or sessions is required; old sessions, run state, caches and old configs
  may be cleaned up, while credentials, raw evaluation records, review evidence, other applications' data and
  Git history are kept. Cleanup runs from an ownership inventory during a switch; this item deleted no data.

The full requirement mapping and acceptance are in the [design baseline](DESIGN.md). The engineering arguments
about the single-database atomic boundary, the runner handshake, the I/O candidates and concurrency defaults
are in the 45-item design review (reachable through Git history: `git log -- review/archive`). Those arguments
are not measured performance results, and they do not mean the user approved each pending library, parameter
or statistical precision. Requirements that conflict with this item's confirmed scope are superseded by it;
untouched behaviour contracts remain in force.
