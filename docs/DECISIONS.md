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

## D-190 The install guide is a user-facing document like the other two (2026-09-27)

`docs/INSTALL.md` was the last user-facing document outside the flag audit, and the reason was in the audit's
own docstring: it was excluded *because it records the flags of earlier releases* — the v0.1.2 note names
`--plain`, `--resume` and `--team` — and a file-level exclusion took the document's *current* commands with it,
the three lines that tell a user to run `teamagents --cwd …`, `exec --json` and the installer's own `--version`
/ `--bin-dir` / `--archive`. Nothing would have noticed if one of those stopped existing, which is the shape
D-135 fixed for the other two documents.

**The audit now covers it**, with the reason for excluding the historical lines turned into a rule instead of an
exemption: a flag on a line that marks itself as history — an older version number (`v0.1.1`, `v0.1.2`) or one
of `earlier`, `legacy`, `removed`, `pre-v2`, `no longer` — is reported as a note, and the window is the line
plus the two above it because prose wraps (measured: the v0.1.2 note carries its marker on line 6 and its flags
on line 9). `--bin-dir` and `--archive` join the toolchain list with the installer as their reason — their own
coverage is `engine/tests/install.rs` and `review/install_check.py` — and the document set is stated once, in
the module docstring: the three documents that tell a user which commands to run are audited, while the rest
quote other products, this repository's scripts, or history.

**Controls** (each reverted): a fresh line `teamagents run --watch-forever` fails with "is not a flag this
build serves"; the same line written as `v0.1.2 had teamagents run --watch-forever` is a note; reverting leaves
the tree at its three historical notes and exit 0.

**And the walk found a sentence the command itself contradicts.** §2 said `init` "creates no session"; the
command prints `session db: …/v2/session.sqlite` because it creates that database, and `init`'s own test
asserts it (`cli::init_creates_private_config_and_never_overwrites_existing_paths`: "init must prepare the v2
root"). The sentence now says what the command does — config mode `0600` with no credentials, state root
prepared with `v2/session.sqlite` created and its paths printed, no daemon started, no conversation opened.

The rest of the guide was walked claim by claim and holds: the platform refusal before any download
(`install.sh`'s `uname` checks), the `gh`-then-`curl` transport, the leading `v` on `--version`, the local
archive requiring the adjacent `SHA256SUMS` and ignoring other platforms' lines, the rollback promise
(`engine/tests/install.rs::failed_second_replacement_restores_both_old_programs`), "never calls sudo and never
edits shell startup files", the musl static archive from the release workflow, the archive carrying
`config.example.toml` (the workflow copies `examples/config.toml` to that name — the phantom path D-109 fixed
in this same document), the bubblewrap table, and "the current version does not probe Codex at all"
(`codex_profile` is a config field no code path reads, D-75).

## D-189 The machine that measures is the machine the leftovers run on (2026-09-27)

D-188 could say "these numbers are load-bound" but not what the load *was*, so this turn measured it. A `ps`
inside the sandbox sees four processes; `/proc/loadavg` reports 3,062 threads on the host. Read from outside the
sandbox (`review/host_cleanup.py`), the host carries **1,399 leftover `jobs-runner` processes**, **1,398 of them
orphaned** (reparented to init) and all but one older than six hours — the exception being the `OUTCOME_UNKNOWN`
runner the A09/A12 test is *supposed* to leave behind, created by this turn's `make check`. None of the old ones
comes from this build, whose runner retires itself when its job settles (D-112) and waits at ~0 CPU when idle
(D-153). Together they hold **5.3 GiB** resident, have burned **665.6 CPU-hours**, and are burning **13.3 cores
continuously right now** (66.4 CPU-seconds in a five-second sample, 347 of them active in it). That is the load
behind D-188's wall clocks and the reason this machine's load average sits at 25-50 on twenty cores.

**What each leftover serves decides its class**, never its age, and the classification is a script rather than
an anecdote — `review/host_cleanup.py` (read-only) reads every runner's job directory and journal:

| class | today | what it means |
|---|---|---|
| `settled-journal` | 1,237 | the journal records a finished command with its exit code (988 `SUCCEEDED`, 249 `FAILED`) |
| `unknown-outcome` | 70 | finished on disk, outcome unverifiable (`CANCELLED`) |
| `dir-gone` | 19 | the job directory it was told to serve no longer exists |
| `unfinished` | 72 | no finish on disk: the command may still be running (`OUTCOME_UNKNOWN` before its finish) — keep |
| `live-parent` | 1 | its parent is alive, so it belongs to a live tree — and on this machine that is **the user's own session** |

The last row is why this census was worth writing before anyone acted on the earlier note: the single runner
whose parent is not init serves `~/.local/state/teamagents/v2`, whose parent is the user's own daemon. A stop
rule phrased as "the terminal-journal and dir-gone set" would have been right; a rule phrased as "everything
whose job finished" would have killed the user's session's runner. So the conservative stop is the **1,256**
`settled-journal` + `dir-gone` processes, `unknown-outcome` is the operator's call (its journal stays on disk
either way, so stopping the process cannot lose evidence), and `unfinished` and `live-parent` are never
candidates. The acting step is one `kill <pid>` per pid taken from a class list
(`review/host_cleanup.py --class settled-journal --pids`); a pattern kill is what D-144/D-148 removed from this
repository and the reason stands — the pattern matches any command line that merely mentions the string.

**The daemons are inventoried too** (3 live): one `user-session` (the user's own root, never a leftover) and two
`root-gone` — one from a crash probe 10.5 h old, one from an isolation test 29 h old, both serving state roots
that no longer exist, which is the stuck shape.

**Two honest notes.** The first is why earlier turns' closing checks could report "0 runners" while this machine
carried 1,399: `review/leak_guard.py` counts the runner family *inside the shell's own PID namespace*, and a
sandboxed shell sees only its own. The guard is right about what it measures — a test that leaks a runner leaks
it into that namespace — but it is not a host census, and the new script says so when it runs inside one. The
second is that the census is a reading, not a control: it records the machine, it does not change it.

**Measured** (2026-09-27): the table above, from one run of `python3 review/host_cleanup.py` plus its five-second
burn sample; `--json` prints the same numbers for a machine-readable record and `--class <name> --pids` prints
one class's pids for the stop. The item stays open until the user says which classes to stop; this entry is its
measured basis, and the script is how the basis is refreshed.

## D-188 A latency number without its machine is not a measurement (2026-09-27)

Refreshing A32's scale evidence at the current build — the row's figures were from 2026-09-25, before D-165's
events index and forty decisions of change — produced numbers that look like a regression and are not one:
append p50 6.1 ms → 8.2 ms, turn step p50 11.1 ms → 14.8-15.2 ms. The machine's **load average was 26-31 on
twenty cores** (it carries the ~1,390 stray `jobs-runner` processes the pre-fix builds left, the standing
host-cleanup item), and both figures are *fsync-bound wall clock*: with `synchronous=FULL` almost all of that
latency is waiting for a commit, so another machine's work is what changed, not the product's cost. The
comparison that isolates it is the probe's own total CPU, which is load-independent: **5.57 s and 5.76 s in two
runs today against 5.50 s on 2026-09-25 (+1 to +5%)**, with the same 10.7 MB of database growth and the same
1.5 ms reopen. Two runs at the same load also reproduce each other (append p50 8.26 / 8.23 ms), so the number
is stable *within* a condition and only moves *between* conditions.

**Changed** (`engine/examples/load_probe.rs`): the report now carries the conditions beside the numbers —
`loadavg_1_5_15` from `/proc/loadavg` and `cpus` from `available_parallelism()` — and the comment above
`cpu_us` says which half of the table is load-bound. That is the whole fix: the previous reports cannot be
compared across machines because nothing in them says what the machine was doing, and the campaign's own rule
("before claiming a falsification, rule out probe error") is what made the difference visible here.

**A32** carries both measurements, the load context and the CPU decomposition, so a reader comparing 2026-09-25
with 2026-09-27 gets the honest reading rather than a phantom regression.

Ceiling: the probe still cannot *control* the load; it can only record it. A measurement taken for a regression
decision should be run on an idle machine (or with the machine's other work named), and `cpu_us` is the metric
to compare when that is not possible.

## D-187 The runtime knew why a completion was refused and did not say (2026-09-27)

The 26-probe model pass (its record is in `review/dogfood/README.md`) came back with one red probe, and it was
worth more than the green ones: `stale_check.py` (A17) failed with "the block reason does not name the stale
input: `['']`" and "the stale-input verdict never appeared in the conversation". The state the probe kept
answered both. The runtime had done its job — `completion_repair` recorded `check bound`, `class
stale_inputs`, `reason "declared input out.txt changed since the check ran"` twice, no goal reported success,
and the goal ended BLOCKED — but the *model* had never been told any of it. Its transcript is the whole story:
a synthetic check round (the check runs `printf changed > out.txt` and exits 0), the answer "[finish received:
required checks round 2 must pass before the goal can settle]", and then eleven requests in which the model
re-read the file, re-wrote it, re-verified it and finally reported the divergence itself.

**The gap.** §8 says "the model is given the real remaining time and error summaries so it can change course",
and the repair path's own comment claimed the receipts were enough: "the failure receipts already sit in the
instance context (decision consumption)". They are — for a check that fails *on its own*, whose output and exit
code land in the transcript as a tool result. The **stale-input class cannot work that way**: the check
succeeded, its output is empty, and only the runtime's re-verification of the declared inputs (below the round)
says why the completion was refused. That verdict was written to the event log and nowhere the model could see.

**Changed.** `repair_completion` (`core/src/v2/control.rs`) now states the verdict in the same transaction that
flips the completion boundary back to READY, as a **note**: `[completion refused: round 1; repair these before
goal … can settle]` followed by one line per failure, `check bound — declared input out.txt changed since the
check ran [stale_inputs]`. The kind matters twice. A note is the established channel for "the runtime tells the
model what was wrong" (D-56) and rides as user-role text without being a user turn; `EntryKind::Runtime` would
have been wrong — its own doc calls it the runtime's *closing* word, the idle rule treats that tail as
committed, and the first version of this change used it: the driver went idle after the repair instead of
asking again, which `v2_driver::the_check_repair_path_keeps_the_transcript_wire_valid` caught by failing to
settle.

**The probe was also wrong**, in a smaller way: `stale_check.py` asserted the *park* path's reason string
(`bound:stale_inputs` in the final `goal_completed`) and searched the conversation for the verdict. The other
live run of the same day settled the goal itself — a different path with the same correct outcome — so the
probe now asserts what must hold either way (the runtime records `class: "stale_inputs"`, the verdict is in the
conversation, no goal is SUCCEEDED, the goal is BLOCKED) and requires the reason to name the stale input only
when the runtime did the parking.

**Measured** (2026-09-27): the new `v2_driver::a_repair_round_names_the_failed_check_and_its_reason` passes, the
wire-validity test passes again, and the whole `v2_driver` suite is 36/36. The probe, re-run live on deepseek at
the fixed build: exit 1 / 16.7 s / 12 requests, "the runtime recorded the stale input: 'declared input out.txt
changed since the check ran'", "the conversation carries the verdict", "the runtime parked the goal naming the
stale input: required checks failed (bound:stale_inputs) after 3 round(s)" — the first live run in which the
model was told.

## D-186 The daemon-stop helper answered "signalled", not "stopped" (2026-09-27)

D-185's `make check` ended red for a reason that had nothing to do with it: `make test`'s leak guard reported a
scratch directory that appeared during the run — `/tmp/ta-tui-knob-778/root/instances/i-leader`, the state root
of D-181's `TEAMAGENTS_TUI` test. The directory had been removed by its `Scratch` guard (D-175) and then
*recreated*, which is the shape D-160 recorded when it added the `Daemon` RAII guard: "a live daemon recreates
the directories it uses, so a removal that raced it left the root behind". D-160 fixed that for a daemon a test
holds a `Child` for (`.kill()` **and** `.wait()`); the sibling helper `stop_detached_daemon`, which the tests
use for the daemon the *engine* detaches, sent SIGTERM and returned at once — "found and signalled" was read as
"stopped", and the very next statement removes the tree that daemon is still writing.

**Changed** (`engine/tests/cli.rs`): `stop_detached_daemon` now waits — bounded at five seconds, polling every
20 ms — until the process is gone or has become a zombie (the container's pid 1 never reaps, which is why the
scan already had that rule). The state-field read is now one `daemon_running(pid)` predicate used by both the
scan and the wait, and it parses `/proc/<pid>/stat` after the parenthesised command name so a name containing a
space cannot shift the field. The D-181 test's second call keeps its deliberately ignored answer, with the
reason written down beside it: the control run boots a session only when the repository's own TUI was found,
and the leak guard is the backstop when there was nothing to stop.

**Measured** (2026-09-27): the test passes 6/6 consecutive runs leaving no `ta-tui-knob-*` directory, and `make
check` is green again ("no leak: 0 daemon(s) and 0 scratch directory(ies) present before the run are still all
there is"). The race was **not** reproduced: with the wait removed the same test leaked nothing in 10
consecutive runs, so the evidence that it exists is the guard's catch — plus D-160's record of the identical
shape — and not a reproduction on demand. That is the same honest ceiling D-161 and D-175 state for their own
races, and it is why the guard, not a test assertion, is what has to keep this closed.

## D-185 The gates assert an outcome; nothing asserted their shape (2026-09-27)

D-184 gave the audits a catalogue. The formal-verification material had the same shape and no gate at all, and
D-159 had already written the sentence that assumed one: "What the gate *does* assert, and this entry did not
change, is the shape: eleven configurations, ten refuted controls, three harnesses." What the targets assert is
an *outcome*: `make verify-model-all` fails when a configuration it names does not verify, and
`make verify-model-counterexamples` fails when a control it names verifies. Nothing noticed a line dropped from
either list, a `.cfg` added to `verification/tla/` that no target runs, a `.tla` module no configuration is run
against, a target naming a file that was renamed, or the counts in `verification/REPORT.md` drifting from the
lists they describe. The material around the gate was the unchecked part.

**`review/verification_catalogue.py`**, in `make hygiene` (fast: it reads text, it does not run TLC), holds the
four together: every `.cfg` and `.tla` a `verify-model*` recipe names exists; every `MC*.cfg` on disk is driven
by some target (verification material nothing runs); every `V2*.tla` is the spec of some target (a model outside
the checked set); every configuration and module is described in `verification/README.md`, the property-by-spec
mapping the report sends a reader to; the report's own two counts equal the sizes of the lists; and the Kani
bullet's quoted run (`Complete - N successfully verified harnesses, 0 failures, M total`) names the number of
`#[kani::proof]` functions the crate actually holds, with `N = M` so the quoted line is internally consistent.
Losing a `verify-model*` target fails the audit instead of passing on an empty set.

**Measured** (2026-09-27): 22 configurations and 10 modules are driven by 4 targets, 22 configurations on disk,
all named and described, and the report's "all **11** configurations" / "all **10** negative controls" agree
with the lists, as does its quoted Kani count (3 harnesses, printed as notes). Controls, each reverted: a new
`verification/tla/MC_orphan_probe.cfg` is
reported as material nothing checks *and* as undescribed; deleting one pair from the counterexample list
reports "`verify-model-counterexamples` drives 9" against the report's **10** and the now-orphaned
configuration; renaming a spec in a copy of the Makefile reports the missing file and the module that is no
longer checked; an extra `#[kani::proof]` function in the harness crate reports "quotes 3 verified Kani
harnesses, but `verification/kani/src` holds 4", and mutating the report's quoted `3`s to `4` reports the
converse.

**The targets were re-run at `f521fd4f` for the report's §0**, which now carries the date, the commit and this
run's wall clock: `make verify-kani` 3 s (`Complete - 3 successfully verified harnesses, 0 failures, 3 total`),
`make verify-model-counterexamples` 1 m 22 s (ten refutations, each naming its property), `make
verify-model-all` 3 m 55 s (eleven times `No error has been found`). Every per-configuration state count is
identical to the run the report already quoted — `MC_task.cfg` 5,721,401 / 606,904 as the largest, `MC.cfg`
84,877 / 18,384, `MC_store.cfg` 48 / 13 as the smallest — which is what a deterministic checker on unchanged
inputs should print, and it makes the point that those numbers describe the *material*.

Ceiling: this audit says nothing about TLC passing a configuration — the states, times and property names in
§0 are measurements of a run, and re-running the targets is what keeps them true (D-159's ceiling). What is
now asserted is what D-159 said was asserted: the shape, and that every file in the directory belongs to it.

## D-184 The audits were the one surface without a catalogue (2026-09-27)

Every documented surface in this repository has an audit that holds it to the code: the events (D-125), the
protocol (D-126/D-173), the tools (D-127), the config keys (D-128), the CLI flags (D-135/D-136), the TUI keys
(D-157). The audits themselves had none. `docs/DEVELOPMENT.md` is the page a contributor reads to learn what
`make check` does, and its paragraph walked through **eleven of hygiene's twenty-two audit invocations** and
stopped: `dead_code` (D-130), `readme_zh` (D-134), `doc_flags` (D-135/D-136), `exec_report` (D-154),
`tui_keys` (D-157), `env_knobs`, `project_config_claim` (D-133), `eval_manifests` (D-145), `eval_surface`
(D-182) and `requirement_trace` (D-137) ran without the page naming them — ten of twenty-two, measured
2026-09-27. Nothing could notice: a script the page never mentions cannot fail a citation, because the page is
prose, and `build_references.py` (D-179) only checks that the *build's* references resolve.

**The page now names every step, and a gate keeps it that way.** `docs/DEVELOPMENT.md`'s hygiene walkthrough is
a list grouped by what each audit protects (the documents, the generated references, what the code does with
what it is given, the build and its evidence, the shell checks), and the new
`review/hygiene_catalogue.py` fails when a script `make hygiene` or `make test` invoke is not named there. It
rules on the two targets the page walks through, refuses when a walked target has disappeared instead of
passing on an empty list, and prints the `review/*.py` the page names that no target runs as a note (four
probe/evaluation entry points are meant to be run by hand).

**Measured** (2026-09-27): before the page was fixed the audit reported exactly the ten missing names, one
finding each; after it, "26 script(s) invoked by make hygiene/test: every one is named in
`docs/DEVELOPMENT.md`", with the four hand-run scripts noted. Controls, each reverted: adding
`python3 review/config_keys.py` to the recipe fails with that script unnamed; deleting the `tui_keys.py`
sentence from the page fails with `docs/DEVELOPMENT.md does not name …`; renaming `hygiene:` in a copy of the
Makefile fails with "no `hygiene` target runs a python script … its rule has stopped applying" — the shape
D-121 calls a silent skip.

**A correction to D-183, found while writing this.** D-183's closing paragraph listed DESIGN §4.4's
"full-disk stop" among the record gaps. It is not one: the clause is implemented and tested — A31 records
`control::disk_full_is_classified_at_the_submit_boundary` and
`v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking`, which drive a *real* `SQLITE_FULL`
through the store boundary and park the instance with the reason. The §4.4 gaps are artifact collection
(D-174) and retention (D-75) only, and D-183 now says so. The mistake is the campaign's own recurring shape
in miniature: a walk that reads a *neighbouring* sentence's subject (the probe's injected-error check) as the
behaviour, where the behaviour had its own two tests one `grep` away.

## D-183 The SQLite version DESIGN requires was checked only by a probe nobody runs (2026-09-27)

Walking DESIGN §3 and §4 — the last prose this campaign had only spot-checked — found §4.4's durability clause
half-wired: "The selected SQLite must include the official WAL-reset fix (3.51.3 and the specific backports),
verified against the version the lock file actually links rather than the crate's declared version." The
comparison existed, as one line inside the probe envelope
(`ensure(rusqlite::version_number() >= 3_051_003, …)` in `engine/examples/probe/suite.rs`), and nothing else in
the tree read it: not `make check`'s 376 tests, not `doctor`, which already reports the store's `journal_mode`
and `synchronous` but not the library those depend on. That envelope is a target no `make` goal runs — only
`docs/DEVELOPMENT.md` names the command that drives it — so a dependency change below the fix would have been
caught only by whoever happened to run it by hand.

**One predicate, three readers.** `core::v2::store::linked_sqlite_carries_the_wal_reset_fix()` answers the
question from the *linked* library (`rusqlite::version_number()`), never the crate's declared version, with
`MIN_SQLITE_VERSION` beside it; `store::the_linked_sqlite_carries_the_wal_reset_fix` asserts it inside
`make check` (offline, no model); the probe envelope now calls it instead of carrying its own copy; and
`doctor`'s `v2 state root` row reports the version and **fails** when it is older, so the durability guarantee
is visible next to the other store facts:

    [ok  ] v2 state root   …/teamagents/v2 (journal_mode=wal, synchronous=FULL, sqlite=3.53.2)

The refusing branch is a private helper with its own test, because a machine that links a new SQLite cannot
produce it any other way.

**Measured** (2026-09-27): the linked library here is 3.53.2 (`libsqlite3-sys 0.38.2`, `bundled`), so the
requirement holds — and now it is also *checked*: `store::the_linked_sqlite_carries_the_wal_reset_fix` passes,
`cli::the_state_root_row_names_the_linked_sqlite_and_refuses_an_old_one` covers both branches (3.53.2 ok;
3.49.1 refused with `lacks the WAL-reset fix 3051003` and the requirement cited), the extended
`cli::init_prepares_the_v2_root_and_doctor_verifies_it` asserts the row carries `sqlite={rusqlite::version()}`,
and `review/test_counts.py` moved the ledger to core 101 / engine 242 / tui 35 (D-178's gate is what demanded
that, and `--write` is what performed it).

Ceiling: a *backport* keeps its old version number, so a patched 3.49.x is refused as too old — the
conservative direction, and recording such a backport belongs in a decision. The check reads the runtime's own
answer, so it says what the binary actually linked and not what a manifest declares.

**The rest of the §3/§4 walk needed no change**: §3's five execution positions and its separation of
`PAUSED/PARKED/TERMINATED` from a phase are exactly `Lifecycle`/`Phase` in `core/src/v2/models.rs`; §4.1's
object list maps onto the sixteen tables (the session *is* the database, its stamp in `meta`); §4.2's commit
points are the control transaction's own tests; §4.3's artifact lifecycle (`STAGING → LIVE → DELETING /
ABANDONED`) is implemented in `core/src/v2/control.rs` with tests. §4.4's remaining sentences — artifact
collection and retention — stay the gaps they are already recorded as (D-174, D-75). Its full-disk clause is
*implemented*, not a gap: `control::disk_full_is_classified_at_the_submit_boundary` and
`v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking` push a real `SQLITE_FULL` through the
same storage boundary (A31). This sentence said the opposite when D-183 was written — see D-184.

## D-182 The evaluation's model-visible surface: claimed frozen, pinned by nothing (2026-09-27)

`review/eval/r2-p6/` is pre-registered evidence and this campaign's measurement of the collaboration
hypothesis, so its treatment has to be the treatment that ran. The harness's own doc comment claimed it — "A/B
keep identical instructions, tools, options and window (the manifest freezes them)" — and checked field by
field the manifests froze two of the four: effort (`model.reasoning_effort`) and the window
(`model.context_window`, D-36). The **instruction text** — A/B's 545-character brief plus group C's
collaboration paragraph, which *is* the experiment's single treatment — and the **offered tool names** were in
no manifest: a one-character edit to that paragraph would have changed group C silently, with all 135 recorded
trials and the H1/H2 conclusions still reading as one experiment.

**What holds it now.** The harness builds its instructions from two named constants and reports, in every
trial's own record, the digest of the template it ran under, the tool names it was offered, the request
options and the limits; `review/eval_surface.py` (in `make hygiene`) checks three anchors against each other:

* the **tree** — the templates are decoded out of `engine/examples/eval_groups_abc.rs` with Rust's literal
  rules (escapes, the `\` continuation that drops the line's indentation, comments), and the tool names out of
  `engine/src/reference.rs::basic_tool_schemas` up to its `if web` block, the shape the harness calls with
  `web=false, skills=false`; if either shape moves the audit refuses instead of reporting a smaller surface;
* the **history** — *every* revision of the harness, its rename followed (the name `rebuild_p6.rs` is gone from
  the tree; the file is `eval_groups_abc.rs` since; five revisions from the pre-registration commit
  `27d7529d` to HEAD) must decode to the same pins, which is how the four 2026-09-24 batches stay covered
  although they predate the field;
* the **records** — every trial that self-reports must report the pinned digest for its group's kind, the
  pinned tools and the pinned effort; and where a trial's committed `session.sqlite` still holds the profile
  the product persisted (`{"model":…,"instructions":…,"options":…,"context_window":…}`), the prompt, the
  effort and the window are read back out of it and compared with the pins.

`freeze.py` computes the pins from those two sources instead of carrying literals, so the manifest and the code
cannot drift apart; `--self-check` exercises the decoder (six literal rules, both source shapes — the inline
`format!` of the pre-registration and today's constants — a mutation that must change the digest, and a source
without the anchors that must be refused). Two mistakes of the first version are recorded in its comments: it
read the recorded prompt with real newlines and matched nothing (inside `session.sqlite` the value is
JSON-escaped, so the reader parses the blob first), and it skipped the state check for batches that predate the
field (which hid the very evidence that later confirmed them).

One change was needed in the driver: `run.py` now writes the manifest's name and digest into
`run-header.json`, so a batch names its own rule instead of being recognised by its task ids. D-145's audit
compares the driver's syntax tree with the pre-registered bytes, so a structural change to it is a failure
there by construction — deliberately, because the driver is the code that produced the recorded verdicts. It
now accepts a difference the manifest **records** in a `driver_changes` entry (D-182 is the first), and prints
it as a note naming the decision and stating that the recorded verdicts came from the frozen bytes; an
unrecorded structural change still fails.

**Measured** (2026-09-27): the audit is green — "5 harness revision(s) and 3 manifest(s) carry one surface" —
and the built harness's own `--print-surface` prints exactly the digests the audit decodes, on both groups.
The controls bite: changing `spawn worker instances` to `spawn worker instance` fails at all three anchors
(the manifest pin, all five harness revisions, and the group-C trials whose committed state carries the old
prompt); prepending a tool to `basic_tool_schemas` fails with "the harness now offers […, `ls_all`, …]"; and
reverting either restores green.

**The batch.** The evaluation was re-run at the productized HEAD to give the new check something to check:
`review/eval/r2-p6/runs/2026-09-27-pilot-bc/` — commit `0fb2624d`, §13's groups B and C, 8 tasks × 1 repeat
with the manifest's own limits (deepseek-flash at its native 1M window, effort `high`, 900 s timeout):
**16/16 trials accepted** (every trial `checks=ok` and `succeeded`), 421,106 real tokens (B 220,692 /
C 200,414), 203.8 s of trial time in a 3 min 33 s batch. It is a *regression* batch, not a fifth round — no
group A arm, one repeat per cell — so it enters no H1/H2 conclusion (`analyze.py` calls B−A "too few samples"
and C−B `+0.000` on it, correctly); what it establishes is that the productized HEAD still passes every task it
ran. All 16 trials self-report the pins, and 11 of their committed checkpoints independently carry the pinned
prompt.

The batch was recorded twice. The first run (same day, same configuration, also 16/16, 418,991 tokens) predates
the surface field, so the re-run *replaced* it: a batch is evidence for the treatment its own records state,
and keeping the older directory would have meant keeping records this audit has to list as "predates D-182".
The two runs' totals differ by under 1% (421,106 vs 418,991) while single cells move by about a tenth in both
directions — the model's own variance on one repeat, which is the honest reason a one-repeat batch supports no
comparison.

## D-181 The knob effects nobody exercised, and a gate for D-180's shape (2026-09-27)

The two candidates D-180 left open, both finished here.

**The knobs whose *effect* nothing tested.** `review/env_knobs.py` keeps the documented knob table equal to the
code's reads, but three knobs had no check that setting them does what the table says: `TEAMAGENTS_TUI` (the
engine's front-end discovery), `TEAMAGENTS_BIN_DIR` (the installer's destination) and the `TEAMAGENTS_ENGINE`
that turned out to be inert (D-180). The first two are documented for users, so they now have effect tests:
`cli::the_tui_knob_decides_which_front_end_the_engine_launches` gives the engine a recorder and asserts the
socket it was launched with (the control leaves the knob unset, where the discovery falls back to the
repository's own TUI and that TUI refuses a non-terminal run — the observable difference between "the knob was
used" and "the default was"), and
`install::the_bin_dir_knob_decides_where_the_installer_puts_the_binaries` installs through the variable alone
and asserts both binaries land there and nowhere else.

**A gate for the shape itself** (`review/flag_fields.py`, in `make hygiene`): for each `struct Args` in the two
CLI parsers it takes the field names and classifies every mention by context — blanking strings and comments
first, then treating `field:` (no `.` in front) as a declaration or literal key, `.field = …` as an assignment,
and everything else (`args.field`, `self.field.method()`, a bare name in a pattern) as a read. A field with no
read at all is a finding, which is exactly D-180's `engine_bin`. It is the flag-level sibling of
`review/command_params.py` (D-124's payload-field check), and it exists because neither rustc's `dead_code` nor
`review/dead_code.py` (public items) sees this shape.

**Measured** (2026-09-27): both new tests pass (0.2 s and 0.1 s); the gate is green on the tree — "21 parsed
flag field(s) across 2 parser(s): every one is read somewhere" — and its **controls** bite: a synthetic
`stale: String` produces one finding, and re-adding `engine_bin` (written, never read) reproduces the D-180
finding verbatim. Writing the audit also reproduced two of this campaign's own lessons in miniature: the first
version read the *declaration* line as a struct literal and reported nothing (a `re.M` slip, D-170's), and the
second counted braces inside strings and swallowed nine thousand characters of `main.rs`, hiding the reads it
was looking for — the committed version blanks string and comment bodies before balancing, and the config
`pub`/`impl` shapes are named in its comments.

## D-180 `--engine` was parsed, documented and read by nothing (2026-09-27)

Checking the last documented surface whose *effect* (rather than existence) no test exercised — the seven
environment knobs `review/env_knobs.py` keeps in step with the code — found that one of them was not merely
untested but inert. `TEAMAGENTS_ENGINE` and its flag `--engine PATH` were parsed by `teamagents-tui`, resolved
by a twelve-line `find_engine_binary` (the explicit path, then the environment, then a sibling, then the repo's
build directories, then `PATH`) into `Args::engine_bin` — and **nothing ever read that field**: the TUI connects
to the session's daemon socket and never starts the engine (`v2_main`'s own comment: "the TUI never executes
anything itself"). Measured 2026-09-27 by reading every use of the field and of the finder: two writes, no
reads, no other caller. The engine even set the variable for its child (`command.env("TEAMAGENTS_ENGINE", …)`),
so a value was manufactured, passed and dropped on every launch.

The class is D-73's — a flag the product cannot honour, silently accepted — and the same file already refuses
its neighbours (`--cwd`, `--full-auto`, `--resume`, `--team`) with an explanation, which is what D-73's own test
(`tui::the_tui_refuses_the_flags_it_cannot_honour`, renamed here) pins. Why no gate had caught it: rustc's
`dead_code` does not fire for a field that is written but never read in these shapes, and
`review/dead_code.py` scans *public* items, so a private dead field and a private dead helper pass both.

**Changed** (`tui/src/main.rs`, `engine/src/main.rs`, `tui/tests/cli_flags.rs`, `docs/DEVELOPMENT.md`,
`Makefile`): `--engine` is **refused** with the pointer its neighbours use ("this binary only connects to the
session's daemon socket and never starts the engine. Start the session with `teamagents` (it boots the daemon)
or `teamagents daemon`"); `find_engine_binary`, the `engine_bin` field and the flag's plumbing are deleted;
the engine no longer exports `TEAMAGENTS_ENGINE` to the TUI; the knob's row leaves `docs/DEVELOPMENT.md`
(`review/env_knobs.py` now reports 6 knobs read and 6 documented) and the Makefile's defensive `unset` drops
the name. The refusal test gained `--engine` and was renamed to what it actually checks.

**Measured** (2026-09-27): `teamagents-tui --engine /x` exits 2 naming the flag and the lever (the test asserts
it), the tree builds warning-free, and the knob audit reports 6/6 with nothing unexplained. Not gated: the
general shape ("a flag stored into a field no code reads") would need a careful textual read/assignment
distinction to avoid false positives, so it stays a recorded candidate rather than a rushed audit — the class
is now covered for this flag by the refusal test, which is the same defence D-73 uses.

## D-179 `git commit -a` skipped the new script, twice (2026-09-27)

The trap D-170 and D-178 each fell into, and the reason both commits needed an amend: `git commit -a` stages
*modifications* to tracked files and skips **new** ones, so a hygiene audit added in the same commit as the
Makefile line that calls it stays untracked — `make check` passes locally (the file is on disk) and a fresh
clone fails at that target. D-170's `decision_citations.py` and D-178's `test_counts.py` were both caught only
by reading `git status` after committing, which is a manual habit, not a gate.

**Changed**: `review/build_references.py`, in `make hygiene`. It reads the surfaces that *execute* scripts —
the `Makefile` and `.github/workflows/*.yml` — and requires every `python3`/`sh`/`bash` invocation of a
`.py`/`.sh` path to (a) exist and (b) be tracked by git. The second half is the trap above; the first catches
the neighbouring failure mode that nothing watched either, a path **typo** in a target. `--surface PATH`
replaces the surfaces, which is how the control is run.

**Measured** (2026-09-27): the tree is green — "32 script reference(s) across 3 surface(s): every one exists
and is tracked" (every hygiene audit, the probes, the pty smoke, `install.sh`) — and the audit **caught its own
introduction**: the first run after wiring it into `make hygiene` reported
`Makefile:196: runs review/build_references.py, which is not tracked by git`, which is the D-170/D-178 defect
stated about itself, before `git add` fixed it. **Controls**: a surface copy that runs an existing-but-untracked
script produces exactly one finding naming the file and the cause; a copy that runs a misspelled path produces
the "does not exist" finding. The audit's own first version flagged `cp install.sh dist/install.sh` — the `sh`
at the end of `install.sh` read as an invocation of the next token — and the lookbehind that fixes it is in the
same commit, with the false positive recorded in the code.

## D-178 The acceptance ledger's headline numbers were stale, and nothing derived them (2026-09-27)

The evidence pass that produced D-176 (probe counts) and D-177 had left the most important document of the set
unchecked: `docs/ACCEPTANCE.md` opens with the precondition every item below rests on — "`make check` is green
(**core 100 / engine 220 / tui 33** test targets) and `make pty` passes" — and both numbers were stale. Measured
2026-09-27 with the crates' own test lists: **core 100 / engine 239 / tui 35**. The line was written on
2026-09-25 and the suites had grown by nineteen engine tests and two TUI tests since. The line's own date was
stale too: the ledger had not been re-dated although D-176's consolidated pass re-ran every gate, and this turn
re-ran `make test`, `make pty` and the live install check.

**Changed**:

* `docs/ACCEPTANCE.md` states the measured counts, and its "checked on" date moves to 2026-09-27 with the pass
  it means named (the gates, the probe sets and the formal gates).
* `review/test_counts.py` (new, inside `make test`) derives the numbers **exactly** — `cargo test -- --list`
  per crate, which runs no test body and needs only the build `make test` already has — and compares them with
  the baseline line, so the sentence cannot rot again; `--write` updates the numbers. It is deliberately *not*
  in `make hygiene`, whose audits must work on a tree that has not been built, and it deliberately does not
  touch the date: "checked on <date>" is a human claim about a review, not a derived value.

**Measured** (2026-09-27): the first run of the check failed on exactly the two stale numbers
(`engine: the ledger says 220, the suite has 239` / `tui: the ledger says 33, the suite has 35`), `--write`
brought the line to `core 100 / engine 239 / tui 35`, and the check is green. **Control**: a copy with
`core 1` and `engine 999` produces two findings naming both sides, and a copy without the sentence produces the
"no baseline line" finding — the shape D-176's probe-count gate uses, applied to the ledger that states the
definition of done's preconditions.

## D-177 `instances pause` printed the new lifecycle beside the old row (2026-09-27)

Verifying DESIGN §6.4's claim that "the UI distinguishes 'pause requested' from 'stopped at a safe boundary'"
— by pausing a real turn with a `sleep 30` command in flight — answered the design question and turned up a
message defect in the same breath. The state sequence is exactly what the design promises, observable through
the two fields the clients print (`ACTIVE / TOOLS_PENDING` → pause → `PAUSED / TOOLS_PENDING` → the command
ends → `PAUSED / READY`), and the waiting headless run reported `timed out: i-leader is PAUSED, so its turn
cannot finish … resume it`, which is D-98's honest line working. What did not work was the lever's own output:
`instances pause --id i-leader` printed

```
PAUSED i-leader: ACTIVE / TOOLS_PENDING
```

— the lifecycle *after* the change beside the row resolved *before* it, which reads as "the pause did not take
effect" at exactly the moment the user is checking whether it did (the D-164/D-171 family: one line saying two
different things).

**Changed** (`engine/src/v2/intervene.rs`, `docs/USER-GUIDE.md`): the pause/resume/terminate line now prints the
row as it is after the lever, in the shape `instances` prints — `i-leader: PAUSED / TOOLS_PENDING`. The phase is
the one the change was made at (a lifecycle change does not move the execution position), which the comment
says, and `instances` shows where it settles. The JSON report is unchanged: `lifecycle` is the new value,
`instance` the row that was resolved.

**Measured** (2026-09-27, after the change): `instances resume --id i-leader` prints `i-leader: ACTIVE / READY`
and the pause prints `i-leader: PAUSED / READY`, against the pre-fix `PAUSED i-leader: ACTIVE / TOOLS_PENDING`.
The existing test
`v2_daemon::the_intervention_cli_cancels_a_task_and_pauses_and_resumes_an_instance` now pins the shape (`…: PAUSED /`
present, `PAUSED i-worker:` absent), which its old `out.contains("PAUSED")` assertion could not.

**And the guide's pause sentence was misleading** (`docs/USER-GUIDE.md` §4.2): it said a pause "stops the
instance at a boundary, the run keeps following its turn, and resuming lets it finish". Measured: the running
turn stops at its next boundary and does **not** finish while the instance is paused — the pause blocks the
turn's next request, so the headless run waits out its deadline and reports it (which is what the D-98 line
does). The sentence and a new bullet now say that, with the `lifecycle / phase` sequence and the reason field
(D-165) a user can watch. Two troubleshooting rows were sharpened at the same time: a state-root FAIL now says
the row distinguishes a file-where-a-directory-belongs (D-166) from a foreign file or a stamp mismatch, and the
"an instance is parked" row says `instances` shows *why*.

## D-176 Two probe counts rotted where nothing read them, and the rule they stated had no gate (2026-09-27)

The consolidated evidence pass after D-163…D-175 (25 of the 26 model probes green, the formal gates re-run,
recorded in `review/dogfood/README.md`) started by re-reading the evidence documents — and both numbers in them
were wrong. `review/dogfood/README.md` said `make probe-offline` runs "the **seven** credential-free ones" (the
offline set has **eight** entries: `boundary`, `budget`, `truncation`, `input_latency`, `tui_panels`,
`tui_reconnect`, `providers --self-check`, `shutdown`), and `review/README.md`'s leak-guard row said "since
D-148 the **twenty-eight** dogfood probes stop their own daemon" (there are 34 entries across the two sets, and
29 probe scripts stop their own daemon). Prose counts are exactly the kind of claim no gate reads: they were
true when written and rotted when the sets grew.

**Changed**:

* `review/dogfood/probes.py --self-check` (inside `make hygiene`) now checks the offline set's *documented*
  size: the phrase `the 8 credential-free ones` must appear in `review/dogfood/README.md`, so growing or
  shrinking the set fails the gate until the sentence is updated. The document states the count in digits for
  that reason, and `--list` prints each set's size next to its header.
* `review/README.md`'s row drops the rotted count for the rule it was about and names the new gate; the
  sentence now says every dogfood probe stops what it started through `review/leak_guard.py` by pid.
* The same self-check refuses an **executable** kill-by-pattern mention anywhere under `review/`:
  `probes.py` parses each Python file with `ast`, drops docstrings (and its own word list, which has to name the
  words) and flags a surviving string containing `pkill`, `killall` or `kill -f`. The prose that explains the
  hazard — in `leak_guard.py`, `crash.py`, the READMEs — is docstrings and comments, so the check is quiet on
  the tree and loud on a re-introduction; D-144/D-148 removed the pattern kills by hand and nothing kept them
  out.

**Measured** (2026-09-27): `--self-check` green on the tree ("selection, budgets, the stray guard, the documented
set sizes and the no-kill-by-pattern rule over 3 sets"), and its own first run caught two things worth keeping —
the stale count (the check exists because it fired) and a **self-reference**: the new word list
`("pkill", "killall", "kill -f")` is itself an executable string, so the tokenizer-based first version flagged
`probes.py` three times; the `ast` version scopes the checker's own function out instead of exempting the file.
**Control**: inserting `PKILL = ["pkill", "-f", "teamagents"]` into `review/dogfood/boundary.py` produces exactly
one finding naming `boundary.py:153`, and the file restored byte-identically is green again.

## D-175 A panicking test skipped its own cleanup, so one CI failure reported as two (2026-09-27)

The wart this campaign hit twice. `engine/tests/cli.rs` builds a scratch tree per test
(`/tmp/ta-<label>-<pid>`) and removes it on the last line — so a test that panics never removes it, and
`make test`'s leak guard reports the leftovers as a *second* failure (D-111's guard fails the run on a leaked
scratch directory, D-147). Measured: the D-171 finding leaked three such trees, the D-174 work leaked two more,
and both times the cleanup was mine to do by hand afterwards. The `Daemon` guard (D-160) had already solved the
same problem for the daemon half.

**Changed** (`engine/tests/cli.rs`): `Scratch` is that guard's sibling — a `Deref<Target = Path>` wrapper whose
`Drop` removes the tree, so every exit path including a panic cleans up. All 24 creation sites now read
`let root = Scratch::new("<label>");` (the pre-test `remove_dir_all` lives inside the constructor), and the
tests keep their explicit final removal, which is now belt-and-braces rather than the only path.
`docs/DEVELOPMENT.md` names both guards where the leak rules are described.

**Measured** (2026-09-27): `make check` green (374 tests, 22 hygiene audits), `make check-nobwrap` and
`make check-broken-sandbox` green (374 each, no `/tmp/ta-*` left), `make probe-offline` 8/8. **Control**:
reverting D-171's tolerance byte-for-byte makes `doctor_predicts_whether_an_mcp_service_can_start` panic under
the no-bwrap condition, exactly the case that leaked three trees before — the run fails with that single test
failure and `/tmp/ta-*` is **empty** afterwards (checked directly, not through the audit, which never runs
because `make test` stops at the failing suite). The file was restored byte-identically.

Ceiling: a test that panics *while a detached daemon is live* can still leave the daemon (the guard is about the
tree); that is the leak guard's half, and it stops and reports those by pid (D-147/D-150).

## D-174 `/artifacts/` was described as shared, and nothing collects artifacts (2026-09-27)

Two defects in the artifact subsystem, found while checking where the `reason` row field of D-165 is documented.

**1. The model was told `/artifacts/` is session-shared; it is per member.** Every model-facing description
said so — `write_file`: "`/artifacts/` is shared by the session: write only deliberate deliverables there",
`read_file`: "shared `/artifacts/` files", `shell`: "Page long output with read_file under private
`/tool-output/`" — and the type called the member's directory `ArtifactPaths::shared`. What the code does: the
driver's root is `<state root>/instances/<id>` (`supervisor`), so `/artifacts/` resolves inside the *member's
own* directory (`ArtifactPaths::own_path` → `resolve_artifact`) and a teammate cannot read it; `/tool-output/`
is a legacy root with **no** producer in this build (every `OutputLocation` is built with the `/artifacts/`
prefix; oversized output is reported as `/artifacts/exec-*.log`), so reading it answers "no private output
directory for this member". The cost is in the tree's own frozen evaluation material: the recorded traces show
models spending turns on the sentence — *"Maybe /artifacts is shared … I'll write to both to be safe"* and
*"Also should I write to /artifacts? … I'll write to workspace root only"* — while a Leader that delegates "write
your report to `/artifacts/report.md`" cannot read the worker's file.

**Changed** (`engine/src/reference.rs`, `engine/src/tools.rs`, regenerated `docs/TOOLS.md`): the three
descriptions now say what the code does — `write_file`: "`/artifacts/` is this member's own deliverable
directory — teammates cannot read it, so put work the team shares in the workspace"; `read_file`: "your own
`/artifacts/` files"; `shell`: "Page long output with read_file under `/artifacts/` (exec-*.log)". The type and
its methods were renamed to match (`ArtifactPaths::own`, `own_path`), the doc comments that called it the
*session* artifact directory were corrected, and the three `artifacts.own.as_ref().unwrap()` sites in the write
paths now refuse a `/artifacts/` path with "no artifact directory for this member" instead of panicking on a
toolkit built without one. New test
`tools::artifact_paths_are_per_member_not_session_shared` pins the semantics: one member writes and reads
`/artifacts/plan.md`, a second member (its own directory) gets an error and never sees the file, and the legacy
`/tool-output/` read is refused by name rather than resolved inside the workspace.

**2. DESIGN §4.4's artifact collection is not applied, and nothing said so.** §4.4 requires "WAL reclamation,
artifact collection and history retention … scheduled separately", A30 requires orphan files to stay
collectable, and the control plane carries the designed step (`artifact_gc_claim`, which marks unreferenced
artifacts `DELETING` and protects every live reference). No build path calls it and nothing deletes a file:
`prune_artifacts` bounds only `exec-*.log` (512 MB per member, measured live), while `resp-*.json` — one per
model response — is kept forever as evidence. History retention is already reported as not applied (D-75);
artifact collection was invisible.

**Changed** (`engine/src/cli.rs`, `docs/USER-GUIDE.md`): `doctor` gains an `artifacts` row when a state root
holds any (count and size, `<state root>/instances/*/artifacts`), saying what *is* done (per-member pruning of
oversized tool output), what is kept (response artifacts as evidence) and what is not (unreferenced artifacts
are not collected). The guide's §6 cleanup bullet carries the same facts. Implementing the collection is a
deletion path through user-visible files, so it stays a recorded gap until the user's word — the same treatment
`[retention]` got in D-75. New test `cli::doctor_reports_the_artifact_footprint` (a row with the numbers when
artifacts exist, no row on a fresh root).

**Measured** (2026-09-27): three real turns in a fresh state root left four `resp-*.json` files under
`instances/i-leader/artifacts/` and `doctor` prints `[WARN] artifacts 4 file(s), 0.0 MB … not collected yet
(DESIGN §4.4)`.

## D-173 The scripting contract's row fields were named in two documents and catalogued in none (2026-09-27)

The user guide's report contract (§1.2) says "the arrays inside a report (`instances`, `tasks`, `approvals`,
`grants`) are rows of the daemon's own views, catalogued with their fields in `docs/PROTOCOL.md`", and
`review/exec_report.py` says the same in its own docstring ("`docs/PROTOCOL.md` keeps the fields of the rows
inside those reports … so this audit deliberately stops at the top level"). Neither was true: `PROTOCOL.md`'s
generated tables listed methods, parameters and reply keys, and its tail was empty — no row catalogue existed.
So the *rows* a script reads had no document and no gate, which is how `reason` reached the instance rows in
D-165 with a decision entry as its only record.

**Changed** (`review/protocol_catalogue.py`, `docs/PROTOCOL.md`): the generator produces one more table from the
same file — **every field the protocol carries in one JSON object**, labelled by the function or read arm that
builds it (`serve_client`'s greeting, `handle`'s replies, `read_snapshot`'s instance rows and `goal`,
`read_events`' rows, and each read arm's rows and envelope). It is built from the `json!({ … })` literals at any
depth in `engine/src/v2/daemon.rs`, so the 19 literals are 19 rows and any field added, removed or renamed in a
protocol object fails `make hygiene` until the document is regenerated. The hand-written prose above it now
points at the table (the sentence the guide's pointer refers to), and the method scan was scoped so the new
table's first column cannot be mistaken for a documented method — a bug the first run of the extended check
reported as four phantom findings (`handle`, `read_events`, `read_snapshot`, `serve_client`: "documented but no
dispatcher arm exists").

**Measured** (2026-09-27): the tree is green ("6 read methods and 40 commands documented and in sync"); the
control — `"sneaky_new_field": Json::Null` inserted into the instance-row literal, then reverted
byte-identically — makes exactly the expected failure appear and then disappear
("the generated tables do not match the code: run `python3 review/protocol_catalogue.py --write`"). The guide's
§1.2 sentence now resolves to real content and additionally names the `reason` field the panel and `instances`
print (D-165).

Ceiling: a protocol object built at runtime is invisible to the table, the same limit the two method tables have;
and the table lists fields, not their types or meanings, which the prose and the code's own names carry.

## D-172 The acceptance matrix had no gate, only the requirement list did (2026-09-27)

§16's definition of done starts with "A01–A36 have automated evidence (uncovered items are listed in
ACCEPTANCE)". The Q half of that ledger has been gated since D-137 (`review/requirement_trace.py`), but the
**A-matrix — the per-item evidence columns themselves — had none**: the two tables were in step by hand (36 rows
each, same ids, verified 2026-09-27), and nothing would have noticed an item dropped from
`docs/ACCEPTANCE.md`, a row for an item the baseline does not list, a row with an empty evidence cell, or the
same item appearing twice (the parse kept the last row and the first became invisible).

**Changed** (`review/requirement_trace.py`, still one audit and still in `make hygiene`): the script now reads
both indexes — `docs/DESIGN.md` §1 (Q1–Q19) and §12 (A01–A36) — and both ledgers in `docs/ACCEPTANCE.md`, and
applies the same rules to each: every baseline id has exactly one row, every row names an id the baseline
lists, no row is a placeholder (a requirement row must cite an acceptance item, a decision or an existing path;
a matrix row must fill both its scenario and its evidence cell), a duplicated row is a finding in either half,
and a missing section is a finding rather than a traceback. `--baseline`/`--acceptance` point at copies, which
is how the control is run (the pattern `decisions_log.py`/`citations.py` established).

**Measured** (2026-09-27): the tree is green — "19 confirmed requirements and 36 acceptance items in the
baseline; 19 requirement rows and 36 matrix rows in ACCEPTANCE.md" — and the control on mutated copies produces
exactly the four findings the rules name: `A01 has more than one row`, `A17 is in the baseline's acceptance
matrix and has no evidence row`, `A14: the row carries no evidence`, `A99 has an evidence row but the baseline's
acceptance matrix does not list it` (`exit 1`). The first control attempt also caught a bug in the new code
(the duplicate message printed `AA01`, because the A pattern captures the whole item where the Q pattern
captures digits); the fix is in the same script.

Ceiling, stated where the script states it: this is coverage and shape, not judgement — a matrix row citing
unrelated evidence passes, and the human who writes the row still decides.

## D-171 The entry document described two things the build does not do (2026-09-27)

`README.md` is what a user reads first, and two of its claims contradicted the tree:

* **"required checks defined by the user or project must actually pass"** (*Completion gate*). The build reads
  the `[[checks]]` of the **user** config only: the project merge loader exists and is unit-tested but no entry
  point calls it (`review/project_config_claim.py`: 0 production call sites), and `config.rs`'s
  `a_project_must_not_be_able_to_install_an_acceptance_command` asserts "only the user's own checks load"
  (`engine/tests/cli.rs` drives the same). The README's own *Configuration and team* section said so three
  paragraphs later, so the document contradicted itself — and the wrong half was the one that suggested a
  cloned repository could impose acceptance contracts on a session, which is the direction the code refuses on
  purpose.
* **"All three wire protocols … compatibility acceptance against five real model services still needs
  environments with the corresponding credentials"** (*Status and limits*). DESIGN §7 keeps **four** protocol
  families apart (Chat Completions, DeepSeek extensions, Anthropic, Responses) and requires "each is accepted
  with a real service separately"; `review/dogfood/protocols.py --self-check` reports "4 families … over the
  wires ['/chat/completions', '/responses', '/v1/messages']", and the probe accepts each family whose credential
  is in the environment, reporting the rest as skipped (D-151). "Three" and "five" appear nowhere in the tree's
  own vocabulary for this (the five *names* are the `protocol` values D-162 serves: `openai`,
  `chat/completions`, `deepseek`, `responses`, `anthropic`).

**Changed** (`README.md` and its Chinese mirror `README.zh-CN.md`, which the repository keeps in step): the
completion-gate bullet says the checks in **your own** config are what must pass and that a project file cannot
add one yet; the status bullet names the four families with their fake-server regression tests
(`engine/tests/providers_fake.rs`), points at `python3 review/dogfood/protocols.py`, and says a family without a
credential is reported as skipped (`--strict` turns that into a failure), so how many are accepted live depends
on the machine.

**And the gate that watches those documents had to be told about the probe's flag**: the new sentence names
`--strict`, which is `review/dogfood/protocols.py`'s, not this product's, so `review/doc_flags.py` (D-135)
refused the README until the flag was added to its toolchain allowlist **with its reason** — the mechanism that
audit's own docstring describes for flags of the tools the docs tell the user to run. Nothing else changed in
either document; `readme_zh.py`'s structural parity still holds (12 headings, 16 links, 18 flags on each side).

**And the same turn found this campaign's own tests coupling to bubblewrap** (`make check-nobwrap`, the CI
condition where `bwrap` is not in `PATH`): three CLI tests were red — `doctor_predicts_whether_an_mcp_service_`
`can_start` and `a_state_root_that_is_a_file_is_refused_by_every_entry_point` asserted `doctor` exits 0, and
without bubblewrap its *isolation* row fails by design (A14), so the verdict is 1 whatever the row under test
says; and `a_leader_parked_under_a_waiting_run_reports_the_park_instead_of_timing_out` used the MCP **workspace**
default, whose server cannot start at all without a sandbox, so the park landed before the client's checkpoint
and the pre-submit guard — not the in-loop path under test — reported it. All three now assert the *row* (the
thing they are about) plus "either doctor passed or its isolation row failed", and the park test's binding is
`mcp_execution = "host"`, which makes the failure timing hold in both conditions. Product behaviour was correct
in every case; the tests were the defect. `make check-nobwrap` and `make check-broken-sandbox` are both green on
this tree (372 tests each), and so is the default `make check`.

## D-170 The comparison document was wrong about this build, and nothing read its citations (2026-09-27)

`docs/PRODUCT-COMPARISON.md` exists for the direction the user set ("reference Codex CLI, pi and hermes"), and
**no audit reads it**: `citations.py` resolves code names and paths, `doc_flags.py` scans README and the user
guide, and the comparison's *TeamAgents* column is prose that only a human re-reads. Re-reading it against the
tree (2026-09-27) found five cells wrong or stale:

* **Sandbox backends** said bubblewrap was "only for shell". `tools::shell_command_spec` → `tools::bwrap_argv`
  is one spec builder used by the shell tool *and* everything dispatched through it — the `[[checks]]`, the
  client's `--check`, and the job runner's commands — and `engine/src/mcp.rs` builds MCP `mcp_execution =
  "workspace"` (the default) from the same argv. `full_auto` and `mcp_execution = "host"` are the deliberate
  opt-outs.
* **Tools** did not say that the web half is offered only for the kinds the config declares (D-168) or that
  search needs a credential (D-167).
* **Config** was missing `instruction_files` and `[retention]`, the two sections that load and are reported as
  declared-but-not-applied (D-102/D-75).
* **Extensions** cited `D-53/D-79` for the user hooks; the binding decision is **D-45** (live evidence D-92),
  while D-53 is a docs-correction entry and D-79 is about the web tools.
* **Headless / CI** cited `D-32/D-49`; D-32 belongs to the *earlier* implementation and is not in this log, so
  the cell now says so ("restoring the v1 D-32 contract that lives in Git history"). `citations.py` cannot see
  this class: a decision *number* is not a path or a qualified name.

**Changed** (`docs/PRODUCT-COMPARISON.md`): those five cells, the column header's date, and — following the
document's own convention for corrected cells — a bullet in its "Honest limits of this snapshot" section that
records what was wrong and why. The comparator cells (Codex/Pi/Hermes) are untouched; their sources are dated
in the table.

**And the class got a gate** (`review/decision_citations.py`, in `make hygiene`): every `D-<n>` in the tracked
markdown must resolve to a `## D-<n>` heading in `docs/DECISIONS.md`, or to a number in that file's "Earlier
rules that still apply" table (the index of earlier decisions that are deliberately not re-stated), unless its
own line marks it as history (`v1`, `earlier`, `removed`, `archive`, `history`, `gone`, `no longer`) — which is
how a documented removal stays legal. `--list` prints both indexes; `--only PATH` checks one file, which is how
the control is run: a synthetic file citing a fabricated number produced exactly one finding while D-42 on the same line
resolved (`exit 1`), and the tree itself is green.

**What the gate immediately found** (2026-09-27, first run): one citation that pointed at nothing —
`docs/DECISIONS.md`'s D-46 entry said the connection graph "replacing D-33's member-to-member restriction", and
D-33 is a **superseded** v1 decision that is in no heading and no index row. The line now says "replacing the
earlier (v1) D-33's …", which is what it means. 128 live decisions and 10 earlier rules are indexed; 0
citations are unexplained.

## D-169 `init` created a directory named `session.sqlite` without saying so (2026-09-27)

The neighbouring typo of D-166, one path segment over. D-166 refuses a `--state-root` that *is* a file; this is
the path that *does not exist yet* and is named like the database file. A fresh root is legal and gets created
(D-149), so `init --state-root …/real/session.sqlite` did exactly what it was asked and produced a directory
called `session.sqlite` with a *second* `session.sqlite` (the database) inside it — measured 2026-09-27, and the
layout then fails its own `daemon`/`doctor` on the parent. Nothing said a word about it.

**Changed** (`engine/src/cli.rs`): when `init` has just *created* the root and its own name looks like a session
database (`session.sqlite`, or any `*.sqlite`), it prints one note naming the shape and the way out — "the state
root is the *directory* that holds session.sqlite and daemon.sock … if you meant that database file, pass its
parent directory next time". It never refuses: creating a fresh root at the name the user chose is the
documented behaviour (D-149), and the note is measured against that — a fresh ordinary directory prints nothing.
An existing root is never noted, since the mistake can only happen while creating one.

**Measured** (2026-09-27): `init --state-root /tmp/…/real/session.sqlite` prints the note (and the odd
`session.sqlite/session.sqlite` layout it would otherwise have left silently); the controls — a fresh ordinary
directory, and that directory used by `exec` end to end — print none. New test
`cli::init_notes_a_root_named_like_the_session_database`.

**And the live model probes were re-run in full** after D-163…D-168, in chunks (`--set models`, 25 probes; the
offline set had already been green): skills, mcp, mcp_http, crash, unknown_outcome, cancel, lifecycle_run,
deadline, job_identity, checks, stale_check, two_gates, exec_check, queued_input, runtime_note, instructions,
hooks, workspace, approval, tui, team_ring, providers, protocols, authority — **all green, no daemons left**,
plus `web.py` (exit 0) and the 8 offline probes. `authority.py` — D-143's recorded exception — passed on its
first attempt, so the shape did not reproduce and its witness did not have to classify a failure.

## D-168 The session offered web tools its config never declared (2026-09-27)

The `skills`-row question ("what is the model actually offered?") answered itself with the surface witness D-143
added. Doctor's row for a config with no `[tools.*]` web entry says "the model is offered neither `web_search`
nor `web_fetch`" — and with `TEAMAGENTS_LOG_SURFACE=1` on exactly that config, the leader's real surface was
`ls,read_file,write_file,edit_file,delete,glob,grep,shell,web_search,web_fetch,skill,wait,send,delegate,spawn`
(2026-09-27, one real turn). Every call to those two would answer `tool web_search is not bound to this member`.
The daemon passed `reference::basic_tool_schemas(true, true)` unconditionally, so the *report* was wrong and,
worse, the offered surface contradicted the rule D-79 recorded as the design in the same breath: "`web_fetch`
and `web_search` are offered **only when the config declares a binding** — that part is the design (§12.1:
binding is the authorization)". D-60 states the same principle for `shell` ("a tool the instance cannot dispatch
is not offered") and only `shell` implemented it.

**Changed** (`engine/src/reference.rs`, `engine/src/cli.rs`): `reference::session_tool_schemas` is the surface a
session's members start from — `basic_tool_schemas(true, true)` with each web kind retained only when the catalog
declares a binding of that kind (`web_search` and `web_fetch` are independent: a config with only
`[tools.fetch]` offers only `web_fetch`). The daemon's leader profile uses it, and a spawned child copies that
profile (`driver`), so the rule holds for the whole team. `skill` stays offered: the `skills` binding is
product-default and the tool answers a capability state when no root resolves (D-167) — which is where the
original question came from, and its `doctor` rows now say so (`the skill tool is still offered and answers no
skills configured`), instead of leaving the user to guess what the model sees.

**Measured after** (2026-09-27, surface witness, one real turn per config): the same config as the baseline now
logs `…,shell,skill,wait,send,delegate,spawn` — neither web tool; the control (`[tools.fetch]` declared only)
logs `…,shell,web_fetch,skill,…` — exactly the declared kind. So D-79's report sentence is true as written, and
the tool catalogue's "profile's tools" label (regenerated) points at the function that decides it.

New tests: `reference::the_web_half_of_the_surface_follows_the_declared_bindings` (nothing declared → neither
tool while `skill`/`shell` stay; one kind declared → exactly that tool; both → both) and the `skills` row
expectations in `cli::doctor_reports_the_skills_registry_and_missing_configured_paths`.

## D-167 `web_search` without a credential called out unauthenticated (2026-09-27)

The `doctor` WARN-vs-FAIL pass the last three entries kept pointing at. DESIGN §7 says "a missing web-search
credential or an unavailable tool is **reported as a capability state**, and no executable binding is
invented", and D-76's ceiling note said "a `required` one still fails the member's start (a missing credential
is a capability state at tool time)". Neither half was implemented. `web_search` read the credential as
`api_key_env.and_then(std::env::var).ok()` and then sent the request **anyway, without an `Authorization`
header** — so an unauthenticated request left the host for a third party, and the model saw the provider's own
401 instead of a sentence naming what to configure. Measured with the control below: `web_search` with an
unset credential returned a *successful* parsed result (`{"provider":"anysearch","query":"q","results":[]}`)
from a local endpoint that needed no auth. A `required = true` binding was no better: `web_tools` failed the
member's load only for an unserved *provider*, never for a missing credential.

**Changed** (`engine/src/tools.rs`, `engine/src/cli.rs`):

* `web_search_credential` is the single wording for the credential state (a variable that is set but *empty*
  counts as absent, the way `config::missing_key_envs` reads one), and `web_search` refuses with
  `web_search_capability` — `web_search is unavailable: credential X is unset (its [tools.*] binding's
  api_key_env); web_fetch needs no credential` — **before** any request is built. The tool call, the member's
  start and the `doctor` row all use that one helper, so they cannot describe the same binding differently
  (D-164/D-166's pattern).
* `web_tools` fails the member's load for a `required = true` `web_search` whose credential is missing, the
  contract an MCP service with an unset secret already had (`bound.rs`); the driver parks that instance with
  the reason and `teamagents instances` prints it (D-164/D-165).
* `doctor`'s web rows follow D-164's rule: a `required` binding that cannot work is a **FAIL** (missing
  credential *or* unserved provider), an optional one stays a **WARN** whose detail says a call answers with
  the capability state. A `web_search` with **no** `api_key_env` at all is now a WARN too (it was `[ok]` with
  "no api_key_env configured", which read as "search works"); `web_fetch` is reported as the half that needs
  no credential, and an `api_key_env` set on a fetch binding is named as unused.

**Measured** (2026-09-27, `make check`'s own unit test, a local `TcpListener`: no third party, no credential):
`tools::a_web_search_without_a_credential_answers_with_the_capability_and_never_calls_out` asserts the
capability sentence, that the listener received **zero** requests, and — with the variable set — that exactly
one request arrives carrying `authorization: bearer test-value`. **Control** (the two changed lines reverted
byte-identically): the same test fails on the first `unwrap_err` because the call *succeeded*, which is the
defect written down. `tools::a_required_web_search_without_a_credential_fails_the_member_load` covers the boot
half (required → `Err` naming the binding and the variable; optional → the binding stays and answers with the
state). The `doctor` rows are pinned in `cli::doctor_probes_isolation_and_config_errors` (FAIL for the required
one, WARN for both optional shapes, the fetch row ok).

Docs: `docs/USER-GUIDE.md` §2 states the credential rule where the `[tools.web]` example is. DESIGN §7 needed
no edit — the code now does what it already said.

## D-166 A `--state-root` that is a file (2026-09-27)

The same audit as D-163/D-164/D-165, on the last path-valued flag. Every entry point joins `session.sqlite` and
`daemon.sock` onto `--state-root`, and the two shapes a user confuses are one path segment apart: the root
*directory* and the database file inside it. Passing the file produced four different raw errnos, none of them
naming the mistake (measured 2026-09-27, real state root): `daemon` and `init` answered
`File exists (os error 17)`, a read verb `Not a directory (os error 20)` with a trailing "point --state-root at
the session you mean", and — worst — `doctor` printed `[WARN] v2 state root not initialized yet (…);
teamagents init or teamagents daemon creates it` with **exit 0**: advice that cannot be followed, because
`init` refuses the same path. D-73's rule ("refused, made to work, or reported as not in effect") again.

**Changed** (`engine/src/cli.rs`, `engine/src/main.rs`, `engine/src/v2/exec.rs`; no new surface): one helper,
`cli::require_state_root_dir`, refuses a path that **exists and is not a directory** — naming the flag, the
shape (the directory holding `session.sqlite` and `daemon.sock`) and the likely intent ("if you meant the
database file, pass the directory that holds it"). A path that does not exist is still legal: every entry
point creates it (`init`, `daemon`, or the client that starts the daemon), which D-149's regression pins. It is
applied where each entry point resolves the root: `prepare_v2_root` (`init`), `daemon_boot`, `doctor` (a FAIL
row instead of the impossible-advice WARN), `handshake` in `exec.rs` (one place covering `exec` and the four
CLI verbs), and `ensure_daemon` (so a client refuses before it spawns anything). Exit codes stay the entries'
own: 1 for `doctor`/`init`/`daemon`, 2 for a client.

**Measured after** (2026-09-27, both a session database file and a plain file): `doctor` exits 1 with
`[FAIL] v2 state root --state-root … is not a directory: …`; `init`/`daemon` exit 1 and `authority`/`exec`
exit 2, all naming `--state-root` and the fix; the control (a directory that does not exist yet) is unchanged —
`doctor` WARNs "not initialized yet", `init` creates it, and `exec` boots a fresh session and answers
(`end=reply`, exit 0). New test `cli::a_state_root_that_is_a_file_is_refused_by_every_entry_point` (0.3 s)
covers all five surfaces plus both controls.

## D-165 The instance list showed `PARKED` and not why (2026-09-27)

D-164's recorded open item, closed rather than left as a known gap. D-164 carried the park reason to the run's
own report (`failure`), the TUI's system note and `doctor`'s static rows, but the surface a user reads *first* —
`teamagents instances` — still printed `id / lifecycle / phase · model` and stopped there, so the user guide's
promise that the list "shows `PARKED` and the reason" was true of neither half of it. The reason was never
missing from the system: the control plane records it in the `instance_lifecycle` event payload (`set_lifecycle`
writes `{lifecycle, reason}` on every transition) and D-104's supervisor test asserts exactly that. What was
missing was the field on the read the clients actually use.

**Changed** (`engine/src/v2/daemon.rs`, `engine/src/v2/intervene.rs`, `tui/src/v2app.rs`, `tui/src/v2ui.rs`):
the checkpoint's instance row carries `reason` — the last lifecycle transition's own words, read with a
correlated subquery over the event log (`reason: null` for an instance that never transitioned, or a
transition recorded without words; no sentence is invented). Every client of the checkpoint gets it without
further protocol work: `teamagents instances` (text and `--json`) prints it for an instance whose lifecycle is
not `ACTIVE`, and the TUI's instances panel shows it the same way. An ACTIVE member's last transition says
nothing a user needs, so those rows are unchanged (`i-leader · ACTIVE · READY · deepseek-flash`), which also
keeps the panel's width.

**Measured** (2026-09-27, real daemon, a `required = true` MCP service whose command does not exist — it parks
the leader's driver at boot, no model call): before, `teamagents instances` printed `i-leader  PARKED / READY  ·
deepseek-flash`; after, `i-leader  PARKED / READY  · deepseek-flash  — required tool service "broken" is
unavailable: MCP workspace initialization failed: MCP server exited`, and `--json` carries the same sentence
in a `reason` field on the `instances[]` row a script already reads. New tests:
`cli::the_instances_list_says_why_an_instance_is_parked` (0.2 s; asserts the
product's own text and JSON output, after waiting for the park over the socket) and
`v2app_tests::the_instances_panel_says_why_an_instance_is_parked` (a TestBackend frame: the reason is on a
PARKED row, absent on an ACTIVE one). `docs/PROTOCOL.md`'s generated tables are unchanged — they catalogue
methods, not row fields, and the document already states that a method's caller "should treat the shape it does
not understand as opaque", so an added row field is compatible by contract.

**The read had to pay for itself** (`core/src/v2/store.rs`): the reason comes from the event log, and the
checkpoint is polled by every client (~4 Hz for `exec` and the TUI), so a kind-filtered scan of an unindexed
`events` table would have been a new per-poll cost that grows with the session. Measured on a 200k-event log
(SQLite, this container): 44.6 ms per read without an index (a full b-tree walk, 20 matching rows) versus
0.02 ms with `idx_events_kind_sequence`. The index is additive — `CREATE INDEX IF NOT EXISTS` in `SCHEMA`, which
the existing-database path already re-applies as its "verify schema" pass — so no format or schema-version
change and a v3 state root opens unchanged (`foreign_tables` counts tables, not indexes, so the foreign-file
refusal is untouched). The write side pays for it: a bulk insert of 100k events went 228 ms → 392 ms (1.71x,
~1.6 µs per event), which is the honest cost of the read being 2000x cheaper at 4 Hz.

**And the guide says what is true again** (`docs/USER-GUIDE.md` §5): the two surfaces that show the park now
show the reason, so the sentence D-164 had to soften is accurate as written.

## D-164 The park reason no client ever showed, and doctor's promise about an MCP service (2026-09-27)

Completing D-104's story. D-104 stopped a driver that cannot boot from taking the coordinator down with it and
parked the instance **with the runtime's own reason** — its comment even named the remaining half: "a headless
run waited out its whole deadline with no event and no log line". Measured (2026-09-27, real daemon, real
session): a `required = true` MCP service whose command does not exist gave `doctor` **exit 0, `[WARN]
tools.broken … (bound at start, required)` under the footer "WARN marks optional capabilities"**, and then
`exec --timeout 60` sat for the whole minute and ended `end=timeout`, exit **124**, `failure: null` — while the
one sentence that says what to fix (`required tool service "broken" is unavailable: …`) sat in `daemon.log`.
The TUI was no better: its `instance_lifecycle` note carried only the lifecycle word, so the reason the
control plane records in the event payload reached **no client at all**. And `doctor`'s static rows did not
predict the boot: a `command` containing `${VAR}` was reported `ok` with "resolves an environment reference at
start", which nothing implements — `mcp.rs` spawns the command verbatim, so `${HOME}/bin/server` reaches
`exec(2)` literally and the service never starts. `doctor` also never looked at the binding's `env` map or its
bearer variable, both of which `bound.rs` reads from this environment and treats as hard errors.

**Changed** (`engine/src/cli.rs`, `engine/src/v2/exec.rs`, `tui/src/v2app.rs`; no new surface):

* `exec` ends the run when it sees the leader parked **under a waiting run** and reports the runtime's reason,
  with the same lever the pre-submit guard names (`parked_fate`, D-164): `end=failed`, exit 1. The park is
  deliberately *not* terminal while a turn is in flight (`phase_now == "RUNNING"`), so a pause keeps D-98's
  behaviour (the driver finishes the current step and a resumed instance still lets the run report that turn's
  own outcome), and a park the user lifted again is re-read every pass (`lifecycle_now`), never assumed.
* the TUI's `instance_lifecycle` note carries the reason (`instance i-leader lifecycle -> PARKED: …`), which
  is where a user watching the session sees it.
* `doctor`'s MCP rows now predict the boot: `required = true` + any condition `bound.rs` refuses is a **FAIL**
  (the instance parks, no member runs), the same condition optional stays a WARN that says the capability is
  dropped; a `${…}` in a command is a failure with the explanation that nothing expands it; and an `env` value
  or `bearer_token_env_var` naming an unset variable is checked the way `bound.rs` reads it.

**Measured after** (2026-09-27, same config): `doctor` exits 1 with `[FAIL] tools.broken … the session cannot
start a member until it is fixed (the instance parks)`; the control (the same binding without `required`)
exits 0, keeps its WARN, and the session boots and answers (`end=reply`, exit 0). With a *required http*
service that fails only after the client submitted (~30 s connect timeout, so the pre-submit guard cannot
cover it), `exec --timeout 120` ends in 30 s with `end=failed`, exit 1 and `failure` = the runtime's own
`required tool service "slow" is unavailable: … connection timed out` plus the resume lever — before this it
was `end=timeout`/124 with `failure: null`. New tests:
`cli::doctor_predicts_whether_an_mcp_service_can_start` (0.3 s), `cli::a_leader_parked_under_a_waiting_run_reports_the_park_instead_of_timing_out`
(4.4 s, a required stdio service whose handshake sleeps then fails), `exec::a_park_the_run_observed_says_why_and_what_to_do`
and `v2app_tests::a_lifecycle_note_carries_the_runtime_reason`.

**Still open** (recorded, not silently dropped): `teamagents instances` lists `PARKED` but not the reason, and
§5 of the user guide said it did — the read model's snapshot carries no park reason (it lives in the event),
so making that true means extending the checkpoint row (`docs/PROTOCOL.md`'s row catalogue and the audit go
with it) rather than stretching the prose. The guide's claim is corrected here to what the code does today.
(**D-165 closed this the same day**: the checkpoint row carries the reason, `instances` and the TUI panel show
it, and the guide's sentence is accurate as written again.)

## D-163 A `--cwd` that is not a directory became the session's workspace (2026-09-27)

Continuing the first-run audit (D-149, D-150) into the *path*-valued flags. `teamagents exec --cwd DIR` is
documented as "work in DIR", and the daemon took the string at its word: `daemon_boot` resolved `workspace`
from the flag with no check at all. Measured (2026-09-27, isolated config and state roots, real daemon):
`exec --cwd …/nope` (a path that does not exist) and `exec --cwd …/afile` (a regular file) both **exited 0**,
and both the `--json` report and the socket's own greeting named the bad path as `session_workspace`. The
session was then unusable in a way that named nothing: every file tool and shell command resolves its root
with `canonicalize` (`engine/src/tools.rs`), so the turn that tried to write a file failed with a bare
`No such file or directory (os error 2)` — the flag was never mentioned. D-73's family again, on a path
instead of a key or a flag.

**Changed** (`engine/src/cli.rs` + `engine/src/main.rs`, no new surface): one helper,
`cli::require_workspace_dir`, refuses a `--cwd` that is not an existing directory, naming the flag, the path
and what the user can do instead. It runs in two places: `ensure_daemon` (at the top, so it covers a session
the client *starts* and one it *joins* — the flag can never be honoured against a live session, and the same
path is what `--check` would run the user's acceptance commands in) and `cli::daemon_boot`, so a hand-started
`teamagents daemon --cwd …` cannot boot that way either. Exit codes stay the entries' own: 2 for a client, 1
for `daemon`. A *different existing* directory against a live session still gets the "did not apply" note
(D-41/D-57); only a path that is not a directory is refused.

**Measured after** (2026-09-27, same roots): all three invocations name `--cwd`, the path and "not a
directory", exit 2/2/1 and leave no socket behind (nothing was started); the control — the same `exec` with a
*real* directory — still boots with `session_workspace` set to it. New regression test
`cli::a_cwd_that_is_not_a_directory_is_refused_before_a_session_starts` covers the missing path, the regular
file, the hand-started daemon and the live-session case; `cli::cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one`
stays green as the control.

**Two riders on the same surface** (same commit, `engine/src/main.rs`):

* the note a second client gets when `--full-auto` cannot apply still said "Stop that daemon (Ctrl-C in its
  terminal)" — the wording D-150 itself declared impossible for the detached daemon users actually run. It
  now carries D-150's recipe instead: SIGTERM the pid, with the `ps -eo pid,args | grep "[t]eamagents daemon"`
  line that names it, so the guide's three instructions and the message agree.
* `approvals` and `instances` printed their unknown-verb message with a literal run of 18 spaces where a line
  continuation was intended; the two messages are one clean line again, like the `authority` and `tasks`
  siblings they sit next to.

## D-162 Two config *values* were ignored the same way (2026-09-27)

D-161 closed the two tables that dropped an unserved *key*; asking the same question of *values* found two more
silent holes. `protocol = "openais"` — a typo — left `doctor` green and fell through the provider dispatch's
catch-all arm, so the session spoke the **chat-completions** wire to an endpoint the user had chosen for another
one; and `[tools.t] kind = "web_fetchx"` kept the tool in `doctor`'s list while the binder's `match` dropped it, so
nothing was ever bound. Both are the D-75 family ("made to work, refused with a pointer, or reported as not in
effect"), and both were measured with `doctor` on an isolated config (exit 0, `[ok]`).

**Changed** (`engine/src/config.rs`): `validate_profiles` refuses a `protocol` this build cannot dispatch (the
set is `openai`, `chat/completions`, `deepseek`, `responses`, `anthropic` — the provider modules' two named arms
plus the shape three names share), and a new `validate_tools` refuses a `kind` it cannot bind (`web_search`,
`web_fetch`, `mcp`), both naming the served set and pointing at the reference. An **empty** `protocol` stays
legal: it is the historical chat/completions default (the base URL still follows `provider`), which several
configs in this tree and the doc's `provider`-only example rely on — the check says so in a comment rather than
guessing a default.

**Measured** (2026-09-27, `doctor`, isolated config): before, `protocol = "openais"` and `kind = "web_fetchx"`
both exited 0 with `[ok]`; after, both exit 1 naming the value and the served set, while all five protocols
(`openai`, `chat/completions`, `deepseek`, `responses`, `anthropic`), all three kinds and an absent protocol still
exit 0. Test: `config::tests::a_value_the_build_does_not_serve_is_refused` (the served set plus both refusals plus
the empty-protocol control). `docs/CONFIG.md`'s third trust rule now says "a key — or a *value*", and the user
guide's §2 line lists `openai` and the absent case, which it had omitted.

**The same day, one more value, and the route each one takes.** `mcp_execution` — the *safety* boundary that
decides whether a workspace-sandboxed MCP service runs inside the member's workspace or explicitly on the host —
had no check at all: `mcp_execution = "workspac"` silently meant the sandboxed default, and `doctor` reported the
binding as a plain `[ok]`. It is now refused at load with its two served values. `mcp_transport` is deliberately
*not*: `doctor` has named an unserved transport since D-74 ("where the user can still fix it without reading a
daemon log", with a test for the removed `sse` value), so this entry's rule is "refused at load **or** named by
`doctor` before a session starts", and `docs/CONFIG.md` now says which value takes which route. Measured after:
`mcp_execution = "workspac"` exits 1 at load, `workspace`/`host` exit 0, and a `stdiox` transport still gets
doctor's `[WARN] tools.t mcp_transport "stdiox" is not one this build speaks (stdio, http)`.

Ceiling: the value sets are validated where the loader knows them (`protocol`, `[tools.*] kind`); values that are
free-form by design (`generation_options`, `[tools.*] env`) stay the service's or the environment's business, and
a typo *inside* a path or a command still only shows up when it resolves to nothing.

## D-161 Two config tables ignored a key this build does not serve (2026-09-27)

D-75's rule is that a key this build does not serve is *made to work, refused with a pointer, or reported as not
in effect* — and most of the config already refused one: an unserved key in `[limits]`, `[retention]`, `[hooks]`,
`[tools.*]`, `[models.*]` or `[[checks]]` fails the load with serde's own `unknown field …`. Two places never
reached that check, and both were measured silent: the **top level** (a key outside `CATALOG_KEYS` was dropped by
the filter that runs *before* parsing) and the **`[permissions]` table** (read by hand, key by key). Concretely:
`skills_pathes = []` — a typo of `skills_paths` — left `doctor` green and the skills path never loaded, and
`[permissions] mod = "full_auto"` — a typo of `mode`, a **safety** setting — silently ran the session in
`approved_scope`. A user who mistyped a budget, a deadline or a permission mode got no signal at all.

**Changed** (`engine/src/config.rs`): one wording, `unknown_key`, is now used by two guards — the top-level table
(against `CATALOG_KEYS`, the set the existing test pins to `UserConfig`'s own fields) and `[permissions]` (against
its two keys, named in the message). Both refuse at load with a pointer to `docs/CONFIG.md`, which gained the rule
as a third bullet in its trust story; the user guide's §2 says the same in one sentence.

**Measured** (2026-09-27, `doctor` on an isolated config): before, `mystery = 1`, `skills_pathes = []`,
`[permissions] mod = "full_auto"` and `[permissions] trust_project_tool = true` all exited **0** with `[ok] user
config`; after, all four exit **1** with `unknown key …`, while a config using the documented keys (including
`mode` and `trust_project_tools`) still exits 0. Test:
`config::tests::a_key_the_build_does_not_serve_is_refused_with_a_pointer` (four refusals plus the positive
control); control: with the top-level guard removed the test fails on its first case (panic in `expect_err`),
reverted byte-identically (`9f64e78b…`).

Ceiling: the rule covers the keys the loader parses as *structure*; a free-form value is deliberately exempt —
`generation_options` is a `HashMap` the provider passes through, so its keys belong to the service (stated in
`docs/CONFIG.md`), and `[tools.*]`'s `env` map is the same shape. And a typo *inside* a string value (a path, a
command) is out of any loader's reach; `doctor` reports the ones that resolve to nothing (missing skills paths,
missing credentials).

## D-160 The leak guard caught its first real leak: a test cleaned up under its own daemon (2026-09-27)

`make check` went red in `make test` with a message that is the whole point of D-147's guard: "leaked scratch
directory /tmp/ta-checks-799 (kept: the directory is the evidence)". The directory came from
`cli::the_daemon_carries_configured_checks_into_the_goal`, which creates `ta-checks-<pid>`, starts a daemon
against a state root inside it, and ended with `let _ = std::fs::remove_dir_all(&root)` — while the `Daemon`
guard that owns that daemon was still alive, because it only drops at the end of the scope. What survived was
`root/instances/i-leader/{artifacts,jobs}`: the daemon **recreated what it uses** after the removal had run.
The `let _ =` is why nobody saw it — a cleanup that fails silently looks like a cleanup — and the leak is a
*race*: restoring the old shape by hand and re-running `make test` passed. That is exactly why the detector had
to be a guard rather than a habit.

**Changed**: `Daemon` gains `stop()` (kill, then wait; `Drop` calls the same method, so it is idempotent), and
all **seven** tests that hold a daemon now call `daemon_guard.stop()` *before*
`std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon")` — the loud form D-111 asked
for. The root can no longer be removed under a live daemon, by construction rather than by luck.

**Measured** (2026-09-27): the failing run's message and leftover shape above; after the change `make test`
reports `no leak: 0 daemon(s) and 0 scratch directory(ies) present before the run are still all there is`, no
`/tmp/ta-*` directory remains, and `cargo test --test cli` is 17/17. The control is stated honestly: restoring
the old shape ran green once (the race did not manifest), so the *evidence* of the leak is the guard's catch,
not a reproduction on demand — and the fix removes the race instead of the symptom.

Ceiling: the other eighteen tests in that file clean up with `let _ = std::fs::remove_dir_all(&root)` too, and
they do not hold a daemon, so their removal is best-effort; the leak guard is their backstop. This is also the
first leak the guard has caught in the field, which is worth recording because it is the argument for having
written it (D-147) rather than a note that counts.

## D-159 The verification report's largest run had the wrong name (2026-09-27)

Re-running the formal gates on the current tree — all 11 configurations, all 10 negative controls, the Kani
harnesses — is evidence maintenance *and* a reading exercise, and the reading found a claim that had been wrong
since the report was written: §0 said "the largest, `MC.cfg`, generated 5,721,401 states / 606,904 distinct".
Those numbers are `MC_task.cfg`'s; `MC.cfg` itself generates 84,877 / 18,384, and the smallest
(`MC_store.cfg`) 48 / 13. Nothing checked the sentence because `make verify-model-all` proves *TLC said
`No error has been found` for every configuration* — its per-configuration state counts are printed for a human
and never compared with the prose that quotes them. The mis-attribution was harmless to the conclusions and
still wrong: a reader who wanted to know which model is the expensive one was told the wrong one.

**Changed** (`verification/REPORT.md` §0): the sentence now names `MC_task.cfg` as the largest with its own
numbers, gives `MC.cfg`'s and the smallest configuration's for scale, states the wall clock of the two gates
(3 m 54 s and 1 m 16 s) and names the properties each negative control refutes — all of it refreshed to
2026-09-27, which is also the date the gates were last run on this tree.

**Measured** (2026-09-27): `make verify-model-all` exit 0, 11 × `No error has been found`; `make
verify-model-counterexamples` exit 0, 10 × a refutation naming its property; `make verify-kani` exit 0, "3
successfully verified harnesses, 0 failures, 3 total" in ~3 s.

Ceiling: the numbers in §0 remain *prose* — the gate cannot compare them with a run (re-running TLC inside
`make hygiene` would cost four minutes per check), so they are dated and illustrative rather than asserted, and
the per-configuration counts stay in the run's own log. What the gate *does* assert, and this entry did not
change, is the shape: eleven configurations, ten refuted controls, three harnesses.

## D-158 Seven environment knobs, one of them brand new, and no place to find them (2026-09-27)

D-143's fix added a knob — `TEAMAGENTS_LOG_SURFACE`, which turns a member's *offer* into a witness — and it
lived in a code comment and one probe's README until the user guide's troubleshooting table got a row this turn.
Writing that row exposed the class: the product reads seven `TEAMAGENTS_*` variables (a diagnostic, two
deployment knobs that locate a binary, and four test-only ones) and nothing connected the list to the code. A
knob nobody can find is a knob nobody uses; worse, the table's *first* version got a row wrong — it listed
`TEAMAGENTS_CHECK_RC_*` as an environment variable, and that string is a **marker the `exec --check` wrapper
writes into a command's output** (`__TEAMAGENTS_CHECK_RC_<uuid>__`), never read from the environment.

**Added**: `review/env_knobs.py`, in `make hygiene`. A *knob* is a whole string literal that names one
(`"TEAMAGENTS_LOG_SURFACE"`) — a literal that merely contains the prefix is not, which is exactly the rule that
catches the marker — collected from `core|engine|tui/src` plus the bare names `install.sh` writes (`${NAME:-…}`,
so it is parsed as a name rather than a literal). Every one must be named by `docs/DEVELOPMENT.md` or
`docs/INSTALL.md`, and no row may name a knob the code stopped reading. Measured: "7 knob(s) read by the code,
7 named by the documents: 0 unexplained".

**Controls**, each reverted byte-identically: renaming a knob in the code fails in both directions ("the code
reads `TEAMAGENTS_LOG_SURFACE_V2` … and no document names it" plus the stale row), and a table row naming a knob
the code does not read fails too.

Ceiling: the audit checks names against names — it cannot tell that a knob's *meaning* changed, which is what the
table's prose is for, and it deliberately does not scan `review/` or `tui/scripts/`, where the probes' own dummy
credentials (`TEAMAGENTS_TRUNCATION_PROBE_KEY` and friends) live and are not product knobs.

## D-157 The TUI's keys had no gate (2026-09-27)

Every documented surface here had a catalogue audit — CLI flags, config keys, events, tools, protocol methods,
the `--json` reports (D-154) — except the terminal keys, which are the surface a user is *inside*. `README.md`
even makes the claim this entry is about: "TUI keys (they match the hint line at the bottom …)", and nothing
checked it: a hint that advertises a key nothing handles is D-130/D-133's shape (a promise with no behaviour),
and the only witness was a human pressing the key.

**Added**: `review/tui_keys.py`, in `make hygiene`. Three lists are compared against
`tui/src/v2app.rs` — the keys the hint lines print (`footer_hint`, the confirmation line, the per-panel hints),
the keys `README.md`'s TUI paragraph documents, and the keys `USER-GUIDE.md` names in its key sections — and
every one of them must have a handler. Only that direction is a finding: a handler no surface advertises
(Home/End, Backspace/Delete, plain typing) is normal. Measured today: "16 advertised and 18 documented key(s)
checked against v2app.rs: 0 unexplained" — the README's claim holds.

Two things the audit's own controls taught it, and both are now part of it:

* a chord is proven by a **window**, not a line: the handlers are nested (`if modifiers.contains(CONTROL) {
  match code { KeyCode::Char('n') => … } }`) and the `Ctrl+A` arm sits 1318 characters from its guard, so the
  first version reported a handler that exists;
* a single letter is proven by a **command arm** (`Char('d') =>`), not by a mention: the quit chord writes
  `KeyCode::Char('c') | KeyCode::Char('d')` inside a `matches!`, which satisfied the first version even after
  the approvals panel's `d` arm was renamed.

**Controls**, five, each reverted byte-identically: renaming the instances panel's `p` arm and the approvals
panel's `d` arm each fail with the hint *and* the document naming them; adding `Ctrl+Z` to a hint fails "the
hints advertise 'Ctrl+Z' and this audit does not know how to prove it"; adding `Ctrl+V` to the README fails the
same way for the document; and naming `F5` in a key line fails too — which also pins the README's own
"deliberately no function keys" claim.

**D-158** then closed the same gap for the environment knobs the code reads, which this entry's turn had
just written down for the first time.

Ceiling: the audit proves a *handler exists* (a control-arm-shaped mention), not that it is reachable in the
view that advertises it — the panel probes do that for the keys a user actually presses (`tui_panels.py`,
`approval.py`). And the documents' key paragraphs are found by a marker heuristic (`ctrl+`, `` `Enter` ``, "tui
keys"), so a key named in unrelated prose is out of scope by construction.

## D-156 The harness named its evidence after its pid, and a later run deleted it (2026-09-27)

`review/dogfood/probes.py` keeps a failing probe's state "because that directory is the evidence" (D-140) under
a root named `teamagents-probe-harness-<pid>`, and removes that root at the end of a run with no failures. This
work runs inside a sandbox with a **PID namespace per tool call**, so every run gets the same low pid (3, 5, …):
the same *name* is handed out run after run. The consequence was measured while writing D-143's note — the kept
session was read, quoted in `docs/DECISIONS.md` and in the A03 row, and an hour later the next clean run had
removed it as its own root. A citation to a path that no longer exists is worse than no citation: D-140 exists
because a failing probe's state was gone by the time anyone looked, and this was the same loss caused by the
very mechanism meant to prevent it.

**Changed**: one place builds a run's scratch root, `harness_root_name()`, and it is `tempfile.mkdtemp` under
`TMPDIR` — unique by construction, never a pid-shaped name two runs can share. The self-check (which `make
hygiene` runs, D-155) now calls it twice and fails if the two names are equal or if the name ends in this
pid; control: with the helper back to `…-<pid>` the self-check reports "the harness root must not be a name two
runs can share: '/tmp/teamagents-probe-harness-5' / '/tmp/teamagents-probe-harness-5'", reverted
byte-identically (sha256 `98560d5b…`).

**Recorded honestly**: D-143's note and the A03 row now say the kept session was read while it existed and was
then deleted by that collision, and that its facts (the grant, the worker's tool list, the two `BLOCKED` tasks)
are what the note rests on. A kept root is still episode-local (`TMPDIR`), so evidence that has to outlive the
run belongs in the repository's ignored `review/tmp/` — which is what D-146 did for its own finding, and what a
future reproduction of D-143 should do.

Ceiling: the fix removes the collision, not the transience — a reboot clears `TMPDIR`, and nothing copies a
failing session anywhere on its own. Making the harness archive a failed run's session into `review/tmp/` is a
small change but one that writes into the repository from a tool that currently writes only to `TMPDIR`; it is
recorded here rather than done unasked.

## D-155 A probe no set ran, and a count that hid it (2026-09-26)

Re-running the model probes after D-153 (the runner lifecycle is what most of them exercise), `--only
deadline.py` selected **nothing**: `review/dogfood/probes.py`'s `--only` matches inside a set, and
`deadline.py` — A35's live half, with its own section in `review/dogfood/README.md` — was in neither set. So
`make probe-models` had never run it, and A35's live evidence rested on a probe the regression run could not
reach. The harness's own documents said "the twenty-four that take a model" while the list held 25, and the
number was both wrong and the reason nobody noticed: a reader comparing a count with a directory has to count
by hand, and nothing checked the file list at all.

**Changed**: `deadline.py` is in `MODELS` (it needs `DEEPSEEK_API_KEY`: one short turn creates the goal, the
probe waits out the goal's one-minute deadline, and the second turn must be refused). `probes.py --self-check`
now requires **every** `*.py` in the directory to be in exactly one set, or named in `NOT_PROBES` with a
reason (the set is empty today: every module here is a probe), and reports the reverse too (a listed name with
no file). `make hygiene` runs that self-check, so the class cannot come back silently in a `make check` — the
harness is one of the four things a user runs to accept this product, and a probe it never runs is worse than
a missing one. The documents' counts became count-free phrasing, and `--list` prints both sets with each
probe's reason.

**Measured** (2026-09-26, the probe's first run *through the harness*): `deadline.py` passes on the current
tree — the first turn exits 0 in 1.0 s with the goal's deadline 59 s ahead, the second exits **1 in 0.0 s**
with `failure: "goal goal-s-main deadline passed before request … could start"`, the session holds exactly one
model request, records `goal_deadline_refused`, and parks the leader with the same words. Controls, each
reverted byte-identically: dropping `deadline.py` from its set fails "deadline.py is in neither set: nothing
runs it"; creating a new probe file nobody added fails the same way for that file.

Ceiling: the self-check proves *coverage* — every probe file is scheduled somewhere — not that a probe is
right, which is what its own controls are for. `NOT_PROBES` is the escape hatch for a future helper module,
and it is deliberately explicit rather than a glob.

## D-154 The `--json` reports were a scripting surface with no catalogue and no audit (2026-09-26)

Five verbs print one JSON object instead of text — `exec`, `authority`, `approvals`, `instances`, `tasks` — and
those objects are what a CI job, a wrapper script or the evaluation harness parses. Four sibling surfaces had a
catalogue audit each (`protocol_catalogue.py`, `event_catalogue.py`, `tool_catalogue.py`,
`config_reference.py`); this one had none, and half of it was undocumented: `docs/USER-GUIDE.md` §1.1 named
four of `exec`'s **sixteen** fields (`end`, `goal_status`, `reply`, `verification`/`watermark` — the rest,
including `failure`, `input_queued`, `permissions` and `verification_path`, were referenced in passing or not
at all). A rename would have broken callers while every test in the tree stayed green — the shape
D-133/D-130 punished twice.

**Added**: `docs/USER-GUIDE.md` §1.2 is the catalogue — one table, `every report` plus a row per verb and its
added fields, with the notes a field name cannot carry (what `end`'s words are, that `goal_status`/`reply` are
the goal's and the member's and never an earlier turn's, that `verification[]` is the check ledger). And
`review/exec_report.py` holds the two sides together, in `make hygiene`: it reads the field names out of the
source — every `json!({ … })` literal in the four report modules that carries `session_id`, which is exactly
what distinguishes a *printed report* from a protocol request (`{"protocol_version", "request_id", …}`), an
event payload or a check verdict — and compares the union with §1.2's table **in both directions**: a field the
reports carry and the catalogue does not name is a finding, and so is a catalogued name no report carries (a
stale name is how a rename hides). The table's rows are the machine-readable side; the prose that follows it
names `end`'s values and the nested fields on purpose, so the audit reads the table only.

**Measured** (2026-09-26): the audit reports "35 report fields, 35 catalogued names: 0 unexplained". Against a
real daemon (a dummy-key config, offline apart from the loopback socket), nine of the reports were printed and
compared with the extraction: `exec` 16 fields, `authority list` 5, `authority grant` 8, `authority revoke` 6,
`approvals list` 3, `instances list` 3, `instances pause`/`resume` 5 each, `tasks list` 3 — **nothing printed
that the extraction does not carry**, and four fields (`approval_id`, `decision`, `task`, `task_id`) are not
printed by any shape that can run offline. Controls, each reverted byte-identically (sha256 `a7991032…` for
the source, `f1e96ea7…` for the guide): renaming `parent_grant_id` in `authority.rs` fails in *both*
directions ("the reports carry `parent_grant_id_renamed` … and §1.2 does not name it" plus "§1.2 names
`parent_grant_id` and no report carries it any more"), and dropping `revoked` from the catalogue fails with
"the reports carry `revoked` … and §1.2 does not name it".

Ceiling: the audit covers the **top-level** fields of the five reports; the rows inside them (`instances[]`,
`tasks[]`, `approvals[]`, `grants[]`) stay `docs/PROTOCOL.md`'s, which has its own audit. Two report shapes
(`approvals approve`/`deny`, `tasks cancel`) are checked by the source extraction only, because exercising them
offline would need a pending approval or task. And the audit cannot see a field whose *value* changed meaning —
only its name, which is what a caller breaks on.

## D-153 Every abandoned runner kept ticking forever, and one machine had 1,397 of them (2026-09-26)

The sandbox this work runs in has its **own PID namespace**, so every guard in the tree — `review/leak_guard.py`,
the probes, `make test` — only ever saw the processes of its own run: "daemons of this run left: 0" was true and
said nothing about the machine. The host process table said something else. On 2026-09-26 this machine held
**1,397 live `teamagents jobs-runner` processes**: the oldest 56.8 h, the median 47 h, every one of them older
than an hour, 19 of them with their job directory *already deleted* (so no client could ever resolve their
socket again). Sampling 200 of them for 6 s cost **9.62 s of CPU** — ~0.8 % of a core each, i.e. **5–11 cores**
of the machine spent on processes with nothing to do. Each was started legitimately: a killed or abandoned
session leaves a runner holding a job whose outcome is unknown, which DESIGN §6.2/§6.3 *wants* (the runner is
the live partner for verification and cancellation, and `crash.py`/`unknown_outcome.py` prove that role). What
nothing bounded is what such a runner does while it waits.

It kept the running-job tick. `TICK` is 50 ms (D-116: 10 ms cost 1.80 % of a core per *running* command), the
serve loop ticks whether or not a child exists, and `Runner::tick` returns immediately when there is no child —
so the whole cost was wakeups, paid forever by exactly the processes nobody would ever talk to again.

Reading the journals of those 1,397 said which shapes they were: **988 `SUCCEEDED`**, **250 `FAILED`**,
**70 `CANCELLED`** — 1,308 runners whose job had reached a *terminal* state and was never shut down, because
D-112's retirement is sent by the driver that is alive to see the receipt — 70 `OUTCOME_UNKNOWN` (the design's
live partner, legitimate) and 19 with their job directory already deleted. No child process was still alive.

**Changed** (`engine/src/jobs/runner.rs`), two rules for an idle runner — no child, no cancel in flight:

* it waits at `IDLE_TICK` (30 s) instead of `TICK`, because the tick's three jobs (a command's exit, a past
  deadline, the TERM→KILL escalation) all need a child, and the whole cost of the old behaviour was wakeups;
* it **exits** when its **job directory is gone** — every client resolves the runner's socket from the token in
  `<job dir>/job.json` (§6.2), so a removed directory cannot be reached by anything the product has, and
  nothing the runner records can be written any more;
* it also **exits one idle cadence after its job settles**. A settled job is fully readable without a runner:
  the journal, the output and the receipt are files (`client::read_output`) and a client that finds no runner
  judges from the persisted journal — which is exactly the recovery path after a crash (§6.3/A11). So the grace
  only has to outlive the client watching the job settle, and that client polls every few hundred ms (D-116).
  This is the rule that retires 1,308 of the 1,397 by itself.

**Measured** (2026-09-26, new idle window in `review/runner_cost.py`, no model, no daemon): the same process in
the same state — no child, no client — costs **0.40 % of a core before and 0.00 % after**; the running-job
windows are unchanged (tick only 0.30–0.40 %, the poll table unchanged), so the child-side latency budget D-116
chose is untouched. Live, with the default cadence: a settled job's runner exits by itself after **29.5 s**
(rc 0), one idle cadence after `finished_ms`. Tests:
`jobs_runner::a_runner_waits_while_its_job_is_reachable_and_unsettled_and_exits_otherwise` covers both rules and
their control (a *reachable, unsettled* job keeps its runner past several idle cadences);
`jobs::tests::an_idle_runner_checks_rarely_but_never_waits_forever` is a **compile-time** pin on the cadence.
Controls, each reverted byte-identically: removing the directory-gone exit fails "runner 735 kept waiting after
its job directory disappeared"; removing the settled-job exit fails "runner 1312 kept waiting after its job
settled"; collapsing `IDLE_TICK` back to `TICK` fails at compile time ("an idle runner must not keep a
running-job cadence").

**And a methodology lesson from those controls**: a control that mutates a source file and runs `cargo test`
leaves the **mutant binary** in `engine/target/debug/teamagents` behind. Two manual measurements after the
controls reported "the runner never exits" and were simply reading the mutant — the source had been reverted,
the artifact had not. A control script that measures anything by hand must rebuild after its revert (the test
itself was never fooled: it compiles what it runs).

**Verified in the field, 2026-09-27**: of the machine's live runners, *every one* ran a **deleted** binary
(`/proc/<pid>/exe` → `(deleted)`, i.e. a build that has since been replaced) and was at least 20 hours old; the
runs made after this entry's build left none at all (the pids of that session are absent from the list). The
count is otherwise unchanged (1,398, 19 with their job directory already gone): the population is pre-fix and
static, which is exactly what the two rules predict — and the one shape they do not retire is a *cancelled*
job's runner, because the cancel path sets `cancel_requested_at` and never clears it, so `idle` is false and the
loop keeps its 50 ms tick forever. That is a smaller version of the same defect (it is what the 68 `CANCELLED`
runners in the population are), and it is fixed the same way: **a finished child clears the marker**
(`Runner::tick`, so the loop becomes idle and the settled/cancelled rules apply). Test:
`jobs_runner`'s cancelled shape in the test above; control, reverted byte-identically
(`f31f5338…`): with the marker left set, "runner 579 kept waiting after its job was cancelled".

Ceiling: a runner whose job directory still exists keeps waiting forever, on purpose — that is the design's live
partner for an unknown outcome, and it now costs no measurable CPU. The 1,397 processes measured here are *old*
binaries: the fix bounds what the next runs leave, not what this machine already carries. The supported way to
retire one is to reopen its session (the daemon imports the receipt and the runner is shut down, D-112); a
lever that stops a state root's runners without reopening it is the same missing product surface as D-150's
daemon stop, and is recorded with it in `docs/ACCEPTANCE.md`'s known gaps. And the sandbox lesson is worth
keeping: an in-run leak guard is evidence about *that run*, never about the machine.

## D-152 A graceful stop with a command in flight: DESIGN §9's sentence, measured (2026-09-26)

DESIGN §9 states what a normal daemon shutdown does — "freezes new dispatch, persists pending work and then
stops itself" — and nothing exercised it. `crash.py` measures the crash (SIGKILL: the runner keeps the job and
the session recovers the verifiable operation from its journal; `unknown_outcome.py` kills the runner too and
gets `OUTCOME_UNKNOWN`), and D-150 made SIGTERM reach the shutdown path at all but asserted only that the daemon
exits 0 and removes its socket. So the question "what happens to a command that is genuinely in flight when the
user stops the session the supported way?" had no evidence behind it, in either direction.

**Measured** (2026-09-26, offline: a local chat-completions server scripts the turn, so no credential and no
network), and now re-runnable as `python3 review/dogfood/shutdown.py` (in `make probe-offline`):

* the daemon stops in **0.3 s**; the client attached to it ends `exit 2` with the transport error
  (`the event stream broke: daemon write: Broken pipe`);
* the operation is **not settled** by the stop — it keeps `DISPATCH_COMMITTED`, because the command is still
  running and the stop cannot know its outcome;
* the **runner survives** and finishes the command on its own, writing its journal (A12);
* the next daemon over that state root **settles the operation `SUCCEEDED` from the runner's journal**, with
  its receipt (`"ok": true`) — A11's "reconnect when verifiable", never a guess and never a replay;
* the command ran **exactly once** (`runs.log` holds one line) and the session stays usable: the goal ends
  `SUCCEEDED` with the user's continuation, and the input that arrives *after* the recovery already settled the
  goal reports its own outcome (`end=unsettled`, exit 1) instead of claiming the earlier settlement — D-72's
  rule, observed rather than assumed.

So the sentence holds, and one detail of it is now precise: "persists pending work" here means the operation
stays open and the *journal on disk* is the record, not that the daemon writes a verdict it cannot know.

**Controls**, each reverted byte-identically (sha256 `6874b30c…`): skipping the stop fails "the graceful stop was
not bounded" plus two follow-ons; a command that leaves no trace and a first reply that calls no tool both fail
at the premise ("the command never went in flight") and then on the trace/replay assertions instead of crashing
— the second control is what found the probe's own `FileNotFoundError` (a probe that throws away its FAIL list
is worse than one that fails). The probe ends with 0 daemons and 0 runners, as the harness guard requires.

Ceiling: one command, one in-flight shape (a *model request* in flight at shutdown is not measured here; the
driver abandons the attempt and the daemon is stopping, so the next daemon's turn is what the user sees — that
is the same `crash.py`/`unknown_outcome` family and is left where those probes measure it). The probe also does
not distinguish "graceful" from "crash" by the daemon's exit status: it cannot reap a child it did not spawn —
that half is pinned by `cli::a_daemon_stops_gracefully_on_sigterm` (D-150).

## D-151 DESIGN §7 asked for a live acceptance per protocol family, and nothing said which ones had one (2026-09-26)

DESIGN §7 separates the protocol layer from the vendor name — "the reusable parsing and contract samples of Chat
Completions, DeepSeek extensions, Anthropic and Responses are kept, and each is accepted with a real service
separately" — and the tree had the first half: `engine/tests/providers_fake.rs` drives a fake HTTP server
through every adapter (six Anthropic tests among them). What it did not have, and what no document said, is the
second half for two of the four families. Live runs existed on the DeepSeek wire (protocol `deepseek`, i.e.
chat/completions plus the `xhigh`→`max` mapping) and on Kimi's `responses` wire; the plain chat/completions
family and the Anthropic family had **contract samples only**, and a reader of A27 could not tell: it lists one
DeepSeek+Kimi team probe. This is D-133/D-130's shape again — the code for a promise exists and is tested, so
prose about it reads like evidence for the promise.

**Added**: `review/dogfood/protocols.py`, one small goal per family (write a file, `--check` verifies it) in its
own state root, with three properties worth naming. It takes each family's native context window **from the
service's own model list** and writes that value into the config it runs with — value and source in D-36's
sense, and the only window source that cannot go stale silently, since a vendor change shows up as a refusal
instead of a quiet shrink. It asserts the design's retention half as well ("opaque provider fields are stored
with their origin and version, and never flattened") by requiring the native field the family's own adapter
stores (`chat_completions.rs`'s `reasoning_content`, `anthropic.rs`'s `anthropic_blocks`, `responses.rs`'s
`responses_output`). And `--self-check` re-derives the endpoint, the native field, the protocol dispatch and the
contract tests from those files, so the table cannot drift away from the code. A family whose credential is
unset is printed as `NOT ACCEPTED LIVE` with its contract tests named; `--strict` makes that a non-zero exit for
a machine that has all of them.

**Measured** (2026-09-26, all four, no `--strict` needed): `anthropic` `k3-256k` over Kimi's
Anthropic-compatible `/v1/messages` — exit 0, `end=completed`, 2 model requests, 7.7 s, `anthropic_blocks`
kept; `chat/completions` (protocol `openai`) `k3-256k` — 2 / 7.7 s, `reasoning_content`; `deepseek`
`deepseek-flash` — 3 / 3.1 s, `reasoning_content`; `responses` `k3-256k` — 2 / 9.2 s, `responses_output`; every
one exit 0 with the artifact and the check passing. The windows both services declare (`/models`:
`context_window` 1048576 for `deepseek-flash`, `context_length` 262144 for `k3-256k`) match the values the tree
already recorded (D-36's user-confirmed 1 MiB and the probes' 262144), which is a second, independent source
for them. Controls, each reverted byte-identically (sha256 `f2406f8a…`): a family pointing at `/v1/wrong` fails
the self-check ("does not post to /v1/wrong"), a family claiming an invented native field fails it, and an
acceptance command that cannot pass refuses the family (`the check did not pass` with the verdict quoted) —
acceptance is the whole path, not a request that returned 200.

**2026-09-27, two re-runs taught the probe what its assertions may claim.** The first re-run was refused by the
**service**: Kimi answered `chat API 429 {"error":{"message":"The engine is currently overloaded…"}}` to all three
attempts of the chat/completions family's turn, and the driver did the right thing (three `Transient` attempts,
then `transient retries exhausted`). The second re-run completed the same family with `exit 0` and the artifact
correct, but its stored assistant message carried no `reasoning_content` — that wire sends reasoning only
sometimes (present 2026-09-26, absent 2026-09-27), so the probe had been asserting a *vendor* behaviour, not the
product's. Both are fixed the same way this repository treats such premises: the probe now re-asks a family once
when the *service* refused transiently (printing both attempts, so nothing is hidden), and it asserts retention
only where it is the product's promise — `anthropic_blocks` and `responses_output` are written by the adapters
themselves, `reasoning_content` is DeepSeek's thinking wire (D-70) — while anything else is required only to be
*unrecognised-free* (a foreign or flattened field still fails). Re-run after both changes: all four families
accepted (anthropic 2 requests / 40.9 s, chat/completions 2 / 35.8 s — with `reasoning_content` present this
time, deepseek 3 / 3.4 s, responses 2 / 34.7 s).

Ceiling: one small turn per family is an acceptance, not a benchmark (no long tool loop, no compaction, no
provider failover), and the Anthropic family is accepted through a **compatible gateway**, which is what §7's
own separation of protocol and vendor describes; Anthropic's own deployment is still not exercised, because no
`ANTHROPIC_API_KEY` exists in this environment — and the `OPENAI_API_KEY` that is present is refused by
`api.openai.com` (`401 invalid_api_key`), so the chat/completions family is accepted through Kimi as well. A
probe detail worth keeping: this container's Python ships its own bundled CAs (conda's `certifi`) and therefore
failed TLS where `curl` succeeded, so the probe prefers the machine's trust store when one is installed
(`review/dogfood/protocols.py`'s `_trust`).

## D-150 The only stop a user could perform was the one that skipped the shutdown (2026-09-26)

Continuing the first-run audit that produced D-149. A user who wants to stop their session has one instruction
in the guide: "stop that daemon (Ctrl-C in its terminal)" — three times (§1, §3, the troubleshooting table) —
but the daemon a *client* starts is detached by design ("starts `teamagents daemon` detached and hands the
socket to the TUI", §1), so there is no terminal to Ctrl-C in, and no verb replaces it: the CLI offers
TUI/`exec`/`authority`/`approvals`/`instances`/`tasks`/`daemon`/`init`/`doctor`/`version` and the protocol has
no shutdown command. The stop a user could perform was therefore `kill <pid>` (SIGTERM) — and `cli::daemon`
installed only a SIGINT handler (`tokio`'s `ctrl_c`, which no detached daemon can ever receive). Measured (2026-09-26, isolated state root): SIGINT →
`stopping...`, exit 0, socket removed; SIGTERM → exit status **15** (the default action), socket **left
behind**, the shutdown never entered. DESIGN §9's "a normal daemon shutdown freezes new dispatch, persists
pending work and then stops itself" was unreachable for exactly the daemon users have.

**Changed** (`engine/src/cli.rs`, no new surface): the daemon waits on SIGINT *or* SIGTERM and enters the same
shutdown, its banner says "Ctrl-C or SIGTERM stops it", and the guide's three instructions become the recipe
that works — SIGTERM to the pid, with `ps -eo pid,args | grep "[t]eamagents daemon"` (which prints each
daemon's `--state-root`) as the way to find it (pattern chosen to avoid `pgrep -f`, whose self-match hazard
D-144 measured). `docs/USER-GUIDE.md` §1 carries the bullet and the other two sites point at it.

**And the probes that meant "crash"** (`crash.py` A08/A11/A12, `unknown_outcome.py` A09) now say so in code:
`review/leak_guard.py` gained `kill_daemons`/`kill_runners` (SIGKILL, no shutdown path) and those two use them,
because a graceful stop is a different scenario from the one they measure — `stop_daemons` (D-147/D-148)
escalates TERM→KILL and would have taken the new graceful path. The other probes that kill mid-run already
used SIGKILL (`os.kill(pid, 9)` in `tui.py`, `approval.py`, `tui_panels.py`, `tui_reconnect.py`,
`input_latency.py`).

**Measured end to end** (2026-09-26, no model): a detached daemon started by `exec` (pid 4) stopped by SIGTERM
in 0.3 s with no survivors, its socket removed and its `session.sqlite` kept; a second `exec` on that root then
started a fresh daemon (pid 18) on the same database and reported the documented `leader … is PARKED` refusal
(exit 2). New regression test `cli::a_daemon_stops_gracefully_on_sigterm` (0.08 s) asserts exit 0, the
`stopping...` line, a bounded stop and the removed socket; control: with the handler removed the same test
fails `ExitStatus(unix_wait_status(15))`, reverted byte-identically (sha256 `141a47b8…`).

**And the client says what that means (2026-09-27)**: making the stop supported exposed the other half — a run
attached when the daemon goes away printed `exec: the event stream broke: daemon write: Broken pipe (os error
32)`, accurate and useless. The client now classifies a lost socket (broken pipe, connection reset, EOF) on all
four paths that can meet one (the event stream, the checkpoint, the submit, the poll loop) and reports it as
`the daemon's socket was lost <what it was doing> (<the transport error>) — the session and its committed state
are kept; start the daemon again …`, exit `2` as documented; a failure that is not a lost socket keeps the plain
wording, because calling every failure a stopped daemon would hide the real one
(`v2::exec::tests::a_lost_socket_is_reported_as_what_it_means`, measured live by SIGTERMing the daemon under an
attached run).

Ceiling: finding the pid is still the user's job. A `teamagents daemon --stop` (or a protocol shutdown
command) is new surface and would have to answer the stale-pid question with the identity machinery the runner
already has (A15), so it is recorded in `docs/ACCEPTANCE.md`'s known gaps as needing the user's word. And
SIGTERM now being graceful means a *crash* is only reachable with SIGKILL. The tree's own tests used
`pkill -f` (SIGTERM) at four detached-daemon sites for the same reason a probe did — a client-started daemon
has no `Child` handle — and they stop by pid now (`engine/tests/cli.rs::stop_detached_daemon`, which finds the
process in `/proc` and skips a corpse); no `pkill` call is left in the tree.

## D-149 The check that creates what it then warns about (2026-09-26)

On a fresh machine, `teamagents doctor` left an empty `<state>/teamagents/sessions/` behind: the row named
"state directory" probed writability by `create_dir_all` on the **legacy v1** path (`config::sessions_dir()`)
and writing `.doctor-probe` in it. The next `init` then printed "found an older release's sessions directory
at … (the old format is not migrated; remove it by an explicit inventory — nothing is deleted automatically)"
— about a directory this build had created moments earlier, inviting the user to clean up the product's own
artifact. The guide invites that order ("doctor … config, credentials, state root, skills, bubblewrap and host
checks"), and the tree's own test already stated the intent the probe broke: "init prepares only the *v2* state
root (R27/A36); it never opens a v1 session, so no legacy state directory appears". `sessions_dir()` has
exactly two callers — this hint and that probe — so nothing else in the build creates the path (`init` alone:
no; `exec` without a config: nothing at all).

**Changed** (`engine/src/cli.rs`, no new surface): the row probes the directory it names, `state_dir()`
(`<state>/teamagents`), and the legacy hint requires a **non-empty** directory. An empty one holds nothing to
migrate, and since anything this build creates is empty, "exists" can no longer be mistaken for "an older
release left it". Both halves have regression assertions in `engine/tests/cli.rs::init_prepares_the_v2_root_and_doctor_verifies_it`.

**Measured** (2026-09-26, isolated `XDG_STATE_HOME`): after `doctor`, the state tree is `teamagents/sessions`
(the defect) and `init` prints the false note; after the fix, `doctor` reports `[ok] state directory
/…/teamagents`, `sessions/` does not exist, and `init` prints no note — while a legacy directory with content
(`sessions/old-session`) still reports and an empty one does not. Controls, each reverted byte-identically
(sha256 `6e4e8885…`): the probe back on `sessions_dir()` fails with "doctor must not create the legacy sessions
layout", and the hint back on `is_dir()` fails with "an empty sessions directory is not a legacy layout".

Ceiling: `doctor` still creates `<state>/teamagents` — the directory its row names — because a writability
probe must write somewhere; the strict read-only alternative (report "not created yet" like the `v2 state
root` row and probe the parent) is a wording decision, not a defect. No other `doctor` row touches the
filesystem (the only `create_dir_all`/`write`/`remove_file` in it is this probe).

## D-148 Every probe stopped its daemon with `pkill -f`, the pattern that killed two shells (2026-09-26)

Each of the twenty-eight dogfood probes ended with the same two lines: a `stop_daemon(state_root)` helper,
registered with `atexit`, whose body was `pkill -f "daemon --state-root <state root>"`. `pkill -f` matches any
command line that *contains* the string, so on 2026-09-26 it matched the shells whose own text mentioned
`daemon --state-root …` and killed two of this session's shells (D-144; three of the probes also pattern-killed
the job runner, and `crash.py` pattern-killed its daemon mid-run). It was also the reason the probes could not
share the guard `make test` uses: the harness had already been fixed to signal by pid, and the probes had not.

**Changed**: the same predicates D-147 extracted now serve the probes. `review/dogfood/*.py` import
`review/leak_guard.py` and call `stop_daemons(<state root>)` — and `unknown_outcome.py` calls `stop_runners`,
the runner family of the same `_pids` predicate (`teamagents jobs-runner <job dir>`), where it used to kill the
runner by pattern. No `pkill`/`pgrep` call remains in any script of this tree; the only mention left is the
docstring that explains why there is none.

**Measured** (2026-09-26): `python3 review/leak_guard.py --self-check` green, including the new cross-family
control ("a daemon is not a runner": `runner_pids(root)` stays empty while `daemon_pids(root)` does not);
`make probe-offline` 7/7 green with "daemons of this run left: 0; new scratch none"; and the two probes whose
kill is *in flight* rather than cleanup were re-run against a real model — `crash.py` printed "killed the
daemon (gone); the client exited 2" and still measured the A08/A11/A12 claims (one line in `runs.log`, the
resumed run `end=completed` / goal `SUCCEEDED` / `input_queued=True`), and `unknown_outcome.py` still measured
A09's (runner stopped by pid, cold recovery marked the operation `OUTCOME_UNKNOWN`, one call of the command,
the task `BLOCKED`). A `grep -rn pkill` over the probes, this module and `tui/scripts` returns the docstring
that explains the rule and nothing that calls it.

Ceiling: `stop_daemons` matches a daemon whose argument list *contains* the state root, which is what the
probes' own pattern did too, but by pid and with the process identity checked (`comm == teamagents`, first
argument `daemon`), so it cannot reach an unrelated process; two probes on the same root would stop each
other's daemon, which is why every probe takes its own `--state-dir`. Stopping still costs up to the guard's
15 s worst case (TERM, then KILL), and `crash.py` needs the daemon dead promptly — measured at ~1 s there,
because the daemon exits on TERM.

## D-147 The leak guard counted and stopped nothing, and it existed twice (2026-09-26)

`make test` and `review/dogfood/probes.py` each answered the same two questions — did this run leave a session
daemon running, did it leave a scratch directory — and each answered them in its own copy of the rule. The
suite's copy stopped neither: it counted `teamagents daemon` processes before and after the three crates and
failed with a number ("the suite left 1 daemon(s) behind"), so the reader still had to find the process, the
daemon kept holding its socket and state root, and the *next* run's baseline counted it. The harness's copy had
already rotted into the measured D-144 defect — a predicate that matched nothing while the guard reported a
leak it could not stop — and the fact that the two copies could disagree at all was the underlying defect:
counting and stopping were two implementations of one rule.

**Changed**: `review/leak_guard.py` is that rule, once, and both callers use it. `make test` takes
`snapshot` before the three crates and `audit` after them; `review/dogfood/probes.py` imports the predicates for
its own end-of-set accounting. The audit reports a *difference* (so a daemon or a directory that was already
there is not this run's doing), names what appeared — pid, and the `--state-root` from its argument list — stops
the daemon it reported (SIGTERM, then SIGKILL, by pid: never `pkill -f`, D-144) and keeps the **state root**,
because that directory is the evidence of the test that wrote it (D-140). The scratch rule is directories only
(D-143's measurement: `TMPDIR` holds `ta-*` *files* written by nothing in this tree, and counting them reports
a leak that is not there). The zombie rule (D-144) is still there, and this entry records that it is now
enforced twice for free: `ps` replaces a corpse's arguments with `<def [teamagents] <defunct>`, so the argument
test excludes it as well — measured while writing the check, which is why the corpse rule is asserted as a pure
function (`is_running`) *and* end to end.

**Measured** (2026-09-26, no model, no credential): `python3 review/leak_guard.py --self-check` — "self-check
ok: the daemon predicate, the zombie rule, the scratch rule and the audit/stop path". Four controls, each
reverted byte-identically (sha256 `0e06faea…`): the predicate replaced by the D-144 prefix test fails three
ways ("did not find a daemon this check started", the audit then finding nothing, and the report naming no pid);
`is_running` returning True fails "a zombie is not a running process: Z->True, Sl->True"; counting `ta-*` files
fails "must count directories only, got […ta-a-directory, …ta-a-file]"; and an audit that reports without
stopping fails "the audit did not stop the daemon it reported". Through the CLI with a leak planted by hand:
`snapshot` then a started daemon then `audit` prints "leaked daemon pid 6 (state root /tmp/ta-guard-control-…/root):
sent SIGTERM, then SIGKILL — gone", keeps the state root, and exits 1; a planted `ta-*` directory prints "leaked
scratch directory … (kept: the directory is the evidence)". `make test` is green with the guard in place and
prints "no leak: 0 daemon(s) and 0 scratch directory(ies) present before the run are still all there is".

Ceiling: the guard sees what is *left* when the suite exits, so a test that starts a daemon and stops it too
late (inside the same run) is not distinguished from one that never started it — that is the per-test guard's
job (`engine/tests/cli.rs`'s `Daemon`, D-111). `make pty` and `make probe-offline` still run their own leak
accounting — `make probe-offline` *is* this harness, and `make pty` needs none (its daemon is an in-process
fake, and `make pty` isolates `TMPDIR` into a directory it removes, D-131) — so nothing is left to unify there.
A daemon the current user cannot signal (`EPERM`) is reported as a survivor rather than swallowed. The probes' own `stop_daemon`
helpers still called `pkill -f` when this entry was written; D-148 moved all twenty-eight of them onto this
module's pid-based stop in the same session.

## D-146 "A check that can never pass" is a claim about the command, and the model can satisfy one (2026-09-26)

`review/dogfood/checks.py` (A16's live half) and `two_gates.py`'s first scenario both configured their
never-passing gate as a workspace-relative file test — `test -f never-written`, run with the session workspace
as its cwd. On 2026-09-26 that scenario did not block at all: `exec` exited **0** with `goal_status:
SUCCEEDED`. The session's own ledger shows the product behaving exactly as designed and the *premise* being
false: `check_round_registered` round 1 → `completion_repair` (`runtime-gate: command exited 1`) → round 2 →
`goal_completed SUCCEEDED`, with the completion summary saying "I created the workspace file `never-written` so
the declared check passes". The file holds one line: "created so the declared runtime-gate check (`test -f
never-written`) can pass". Nothing was bypassed — the check ran, failed, and the model then made it pass, which
is what a *satisfiable* acceptance criterion looks like. The gate was never broken; the probe's word "never"
was, and the same latent defect sat in `checks.py`, whose docstring even named a path
(`review/dogfood/never-written`) its own `CHECK` constant did not use. Worse, the probe had been *passing* for
the wrong reason: in its D-101 run the same check held only because that model chose to concede
(`blocked`) instead of writing the file, so the "impossible" premise was never tested.

**Changed**: both probes now configure `command = "exit 1"` — a bash builtin whose status no workspace content
can change, so the gate cannot pass by any action the model could take — and the reason is stated in the probe,
not in the generated config. The generated config is *model-readable*: in the first fixed run the model quoted
the probe's own comment back ("its own comment states this check 'cannot pass in any workspace state'") while
deciding not to report success, so the wording now says only that the entry is the runtime's gate
(`review/dogfood/checks.py::config_text`). A probe that writes its conclusion into the scenario is prompting,
not observing.

**Measured** (2026-09-26, native windows per D-36): `checks.py` on **deepseek** 10 model requests / 16.2 s and
on **kimi** 9 / 36.9 s — `end=failed`, exit 1, goal **BLOCKED**, **3 check rounds**, 2 repairs, settlement
`blocked_by: runtime`, the ledger naming `check_id: impossible` / `class: exit`, `hello.txt` exact in both, and
0 job runners left (A12); `two_gates.py` scenario 1 exit 1 / goal `BLOCKED` / client verdict `ok: true` in
16.0 s and scenario 2 goal `SUCCEEDED` / exit 1 in 5.2 s. The comment is not cosmetic: with the editorializing
wording the deepseek run conceded after **one** repair (goal `BLOCKED`, `exec` 1, the candidate's own blocked
finish) instead of exhausting the rounds; with the neutral wording both providers kept claiming success and the
runtime blocked the goal itself. The probe now *prints* which of the two settled it
(`checks.py`'s "settled by ..."), because both are honest routes to the same promise and only one of them is
the runtime's decision.

**Kept as evidence**: the failed run's session — the model's summary and the `never-written` file — quoted
verbatim, with the four ledger events and the goal row, in `review/tmp/d146-two-gates-satisfiable/` (its
`README.md`). The harness's own copy of that session lived under a pid-named root and was deleted by a later
clean run (D-156); the durable copy above is what the record rests on, which is why it was made. The model's own `evidence` list records the reasoning — it checked that the file satisfied the
check — and its `unverified` list says "Whether the runtime executes the check with cwd = the workspace; I
verified it passes from the workspace root only".

Ceiling: a check the model *can* satisfy is not a product defect — `[[checks]]` are the user's acceptance
criteria, and whether a criterion can be gamed is the user's choice of criterion (Q11); what is not allowed is
a probe claiming "impossible" without a command that is. And a gate no workspace state can satisfy is
conceded by a model often enough that the runtime's own block is worth driving deliberately: the exhaustion
path stays pinned by `v2_driver::required_checks_exhausted_parks_the_goal_blocked` as well as by the probe.

## D-145 The pre-registered analysis no longer matches the tree, and nothing said so (2026-09-26)

`review/eval/r2-p6/` is pre-registered evidence: all three manifests pin the analysis script's sha256 ("frozen
before the run") plus every task's prompt, fixture and check-list digests, at a frozen date and commit. Nothing
checked that the tree still matched, and it does not: the current `analyze.py` hashes to `07f85926…`, the pin is
`52d257b4…`. The cause is benign and the *claim* was the problem: `478d679` ("English verification material")
translated the script into English under the repository's language rule, so the file is no longer the bytes the
pre-registration names — silently, so a reader who verifies gets a mismatch and cannot tell whether the
*analysis rule* changed.

It did not. The frozen bytes are recoverable from the repository's own history — the commit `27d7529` that ran
the trials holds a version hashing to exactly `52d257b4…` — and the two are **AST-identical with every string
literal replaced** ("<text>"), so only comments and printed text differ. The manifest's `git` field turns out not
to name those bytes at all: at `eda3b56e` the files did not exist ("new file" in `git diff`), which is why a
naive `git show <frozen>:<path>` recovery fails. The rule is therefore intact and the digest divergence is a
*translation*, not a change of method — but only because someone looked.

**Added**: `review/eval_manifests.py`, in `make hygiene`. For every manifest it recomputes each task's
`prompt.md` digest, fixture tree digest and `checks` list; it recovers the pre-registered bytes **by digest**
(the newest commit of that file that hashes to the pin) rather than trusting the manifest's commit field; and it
compares the analysis script and the driver against those bytes by AST with strings stripped — so translated
text is a *note* ("its digest differs from the recorded one (its text was translated), and its rule is identical
to `27d7529`") while a changed rule is a **failure**.

Evidence: the audit reports the three manifests, six notes (analyze.py and run.py, once per manifest) and exits
0. Controls, each reverted byte-identically: appending one newline to `tasks/edit-integrity/prompt.md` fails with
"edit-integrity prompt.md no longer matches its digest"; changing `analyze.py`'s sort to `reverse=True` fails
with "differs from the pre-registered 27d7529 in its *rule*, not only in its text" for all three manifests.

Also updated in `review/eval/r2-p6/REPORT.md`: the note above the conclusions, so a reader of the evaluation
record learns this before trying to verify the digest themselves.

Ceiling: "the rule is identical" is a statement about syntax trees — a translated format string cannot change a
number, but the audit cannot prove the *numbers* the frozen script would print today; the recorded verdicts
stand because they were produced by the frozen bytes at run time. And the audit needs the frozen bytes to remain
in history.

## D-144 The probe harness checks its own rules (2026-09-26)

The harness that runs the probe sets accumulated rules of its own, and this session showed three times that a
check counting the wrong thing is the expensive kind of defect — D-130's mentions that were not calls, D-140's
"runs" that were calls of a *different* command, and D-143's witness that counted the leader's `delegate` as a
shell attempt. Its rules are now stated in a runnable check, with no model, no daemon and no probe run:

    python3 review/dogfood/probes.py --self-check

It states four things: the stray guard counts **directories** (plain files with the `ta-` prefix appear in
`TMPDIR` from elsewhere on this machine — measured: `ta-cap-stdout`, `ta-cap-stderr` and an earlier
`ta-wide-d71.log`, none of which this tree writes — and counting them reports a leak that is not there);
`select()` returns a set unchanged, selects *every* entry whose name was asked for (so `providers.py`, which is
deliberately in both sets with different arguments, yields two for `--only providers.py`), and selects nothing
for a name that matches nothing; every set has a per-probe budget (`set(SETS) == set(TIMEOUTS)`, so a new set
cannot silently fall back); and only the model sets need a credential.

Both `make probe-offline` and `make probe-models` run the check before their set, so a harness rule that breaks
is reported before twenty-four probes have spent real calls.

Evidence: `--self-check` prints "self-check ok: selection, budgets and the stray guard over 3 sets" and exits 0,
and two controls, each reverted byte-identically, make it fail — counting files in the stray guard gives "must
count directories only, got ['ta-a-directory', 'ta-a-file']", and dropping a set from the budgets gives "every
set needs a per-probe budget: sets=['all', 'models', 'offline'] timeouts=['models', 'offline']".

**The full set then failed on the guard itself**, which is the same lesson once more: twenty-four probes ran,
**23 passed** (`authority.py` on its slow path at 587 s — the retries from D-143 doing their work) and the run
was red for `1 daemon(s) left running`, with *no* failing probe. The guard's TERM-then-KILL sweep only covered
failed probes; a probe that passed relies on its own `atexit`, which sends TERM only, and a daemon slow to
honour that left the set red. The guard now applies the same sweep over its own root — where every probe's state
root lives — when the count has still grown after its wait, and reports only a survivor. Control: a daemon
started by hand under a fixed harness root, with the baseline patched to look grown, is gone by the end of the
run (`daemons 0 -> 0`), which is what the sweep is for.

**And the lesson a fourth time, on the next full set**: all **24 probes passed** and the run was *still* red —
`1 daemon(s) left running`, from a **global** count of `2 -> 3`. The daemon was not the run's: counting every
daemon on the machine attributes another session's (or an earlier root's) to this one, which is precisely the
stray-*file* false positive again, inside the same guard. The check is now attributed to the harness's own root
— the count, the sweep and the report all ask "under `<harness root>`" — and since every probe is given
`--state-dir <root>/<probe>`, nothing the run starts is missed and nothing else is blamed. Control: a daemon
started by hand under a *different* root leaves the run green with `daemons of this run left: 0`, and it is left
untouched.

**The daemon check had four shapes of false positive, and each was measured before it was fixed**: the *stray
files* with the `ta-` prefix that this machine writes from elsewhere; the *foreign daemons* a global count
attributed to the run; the *zombies* — `crash.py` kills its daemons on purpose and an orphaned corpse stays
`<defunct>` here, keeping the binary's name; and, worst, the *patterns*: `pgrep`/`pkill -f` match any command
line that *contains* the string, so `daemon --state-root <root>` also matched the shells whose text mentioned it,
and two of this session's own shells were killed by it. The guard now counts live processes by `ps` state,
attributed to its own root, and stops them **by pid** (`os.kill`), never by pattern. It also names any survivor
with its argv, and allows a stopping daemon a minute before calling it a leak.

**And a fifth shape, the worst of the family**: the sweep's predicate matched **nothing at all**. `ps` prints
the `args` column starting with the binary's *path* (`engine/target/debug/teamagents daemon --state-root …`), so
`args.startswith("daemon ")` was never true — the guard reported the daemons it could not stop, and every "daemon
left" line this session was a *real* leftover that the sweep had silently failed to touch. A daemon is recognised
by its **first argument** now (`args.split()[1] == "daemon"`), which is what `make test`'s own guard has always
checked (`ps -eo comm,args` with `$3 == "daemon"`), and `daemons()` counts what `daemon_pids()` finds so the two
can never disagree. Verified against a daemon started by hand under a harness-style root: the predicate finds it
(pid and argv), `daemons()` agrees with itself, and `stop_daemons` brings it to zero — and `crash.py` through the
harness now reports `daemons of this run left: 0` with nothing left behind, which it never did before.

Measured while resolving it: that daemon outlives its probe by minutes when stopped only with SIGTERM (the
probes' own `stop_daemon` helpers sent TERM only then; D-148 gave them this guard's TERM-then-KILL), and the
harness's TERM-then-KILL stops it — whether the daemon
ignores TERM or its shutdown waits on the outstanding `OUTCOME_UNKNOWN` runner is not established, and the same
`SIGTERM` question applied to `make test`'s guard, which counted rather than stopped (D-147: it stops what it
catches now, with the same escalation).

**D-150 changed the signal semantics below this observation**: SIGTERM is the graceful stop now and an abrupt
end is SIGKILL, so a daemon that outlives a TERM stop is a case this entry did not re-establish.

Ceiling: the check covers what can be stated without a session. The keep-on-failure path needs a real probe run,
and the daemon check's remaining question (the paragraph above) needs the next `crash.py` run with its state
kept.

## D-143 The two probes that failed the sweep now say which shape they saw (2026-09-26)

Three of D-141's failures were reported as bare assertions, and a bare assertion is what made them expensive to
diagnose. Each now names the evidence it has.

**`workspace.py`** (D-142) is green in **8 of 8 runs** after the pair-wait, so the race is closed; a failure now
dumps `git worktree list` and which of the two records survived, because the one *unexplained* shape — the
directory gone, both records present, and only the earlier refusal in the daemon log — is precisely what that
dump settles. It did not recur in those eight runs.

**`authority.py`** had two problems in one assertion.

*Turn 1* has to close (the leader spawns the worker, delegates the question and reports the answer), and one run
instead spent **thirty model requests** and ended `got 124`: the worker answered in prose without settling its
task, so the leader's `wait` stayed pending until the turn hit its own deadline. That is the gap
`docs/ACCEPTANCE.md` has recorded since this session's first turn ("a model that stops settling its task leaves
a visible wait", D-129) and not anything about the authority surface. The probe now sets the turn up again once
(resetting the state root and stopping the daemon in between) and, if it cannot, reports the gap as the likely
cause instead of a bare exit code.

*Turn 2's* assertion — "the worker still could not run the command after the grant" — conflates three shapes,
so a failure now prints them: the worker's open task and request count, and every operation whose arguments
name the command's artifact. **No attempt** means the surface never offered the tool, a **refusal** is a
grant/dispatch question, and an **open task** is the recorded gap again.

That distinction is not academic, because one run in five showed the first shape: the grant was issued
(`revision: 8`, subject `worker_shell_check`, scope `workspace`) and the worker's next turn reported its tool
list *without* a shell tool and never ran the command — while a run twenty minutes earlier ran it and a later
run ran it in 621 s. I have not explained it, and I am not guessing: the probe's scratch had already been
removed at exit, so the evidence is gone (the D-138 policy; `--state-dir` keeps it). **Open**: does a
`shell@workspace` grant always reach the next request's surface, or is there a window in which it does not?
The next occurrence answers it directly from the message above.

Also measured, and the reason the harness budget moved: `authority.py` took **21 s** in one run and **621 s** in
another on the same build, because the model chooses how long its turns are. The models set's per-probe budget is
1800 s (each turn is still bounded by the probe's own 600 s) rather than reporting a slow model as a failure.

Both new paths are exercised by a control, each reverted byte-identically: treating a *closed* first turn as a
failure makes the probe set the premise up twice and print the gap note both times, and making the artifact check
unsatisfiable prints the new message with the worker's own attempts — `task=none after 9 requests,
attempts=[('6dbe4b:0', 'SUCCEEDED'), ('8d4a8d:1', 'FAILED')]` in that run, i.e. the worker *had* run the command.

**The grant question in that message is answerable from the code, so the message now answers it** with
`grant=live` or `grant=ABSENT`: `driver::team_kernel` rebuilds a member's profile for **every** request and reads
the grants live on the single-writer connection, dropping `shell` only when no covering grant is held — so a
request built after a grant always offers the tool, and *no attempt with a live grant* means the model chose not
to use it rather than never seeing it. Both paths of the new helper are validated: `live_shell_grant` answers
False on the earlier successful run's own session (its grant was revoked at the end — `revoked_at` set and a
`grant_revoked` event) and True on a copy with the revocation cleared, and the sharpened message prints
`grant=live, task=none after 11 requests, attempts=[… five SUCCEEDED …]` under the artifact control.

**The rule the reading rests on is pinned by tests, not only by reading code** (a correction to this entry's
first draft): `v2_supervisor::the_offered_surface_follows_the_grants` covers the direction without a grant (a
spawned worker holds no `shell@workspace` and every call is refused), and
`v2_supervisor::a_users_grant_reaches_the_workers_surface_at_the_next_request` covers both directions of the
change — the worker's first request carries no `shell`, the next one after an `issue_grant` **does**, and every
request after a revoke does not — all issued through `submit_user`, the same single-writer path the daemon's
`authority` client uses. So "grant=live and no attempt" is a model choice on verified behaviour.

Ceiling: what the *run* cannot show is the schema list that went out with a request — that surface is assembled
per request and only the instance's *configured* profile is persisted — so a record of what a member was offered
would be a candidate addition rather than something this probe can read; it is recorded with the known gaps in
`docs/ACCEPTANCE.md` because it is new persisted surface and needs the user's word. A run reporting
`grant=ABSENT`, or an attempt whose receipt is a refusal, remains the product-side finding this watches for.

**The live occurrence, later the same evening** (`make probe-models` again) printed exactly that message and then
showed the witness was too loose: `grant=live, task=none after 10 requests, attempts=[('13d4b5:0', 'SUCCEEDED')]`
— and that operation is the leader's **`delegate`**, whose task description names the command, so matching on the
artifact alone counted a delegation as a shell attempt (D-130's and D-140's trap, a third time). `shell_attempts`
now requires the intent's `name` to be `shell`; against that state it returns `[]`, and against the earlier
successful run's it returns three. The shape is therefore settled: the grant was **live**, the worker **never
attempted** the shell, and the task the leader had just delegated was still `PENDING` when its turn ended — the
leader had reported the worker's *earlier* (pre-grant) answer and did not wait for the new task. That is the
model's behaviour, with no evidence anywhere in the run that the tool was missing, and the same build ran the
command in two other runs.

Because it is behaviour rather than a defect, turn 2 now gets the same bounded retry turn 1 has: if the artifact
is not there after the first ask, the probe asks again **with the grant still in place** (the session is not
reset), and prints a line saying so. A run of the probe after the change passed in 28 s with the first attempt.

The harness's leak guard also learned some precision: it counts `ta-*` **directories** now, because plain files
with that prefix appear in `TMPDIR` from elsewhere on this machine — measured: an empty `ta-cap-stdout` and
`ta-cap-stderr`, and earlier a `ta-wide-d71.log`, none of which this tree writes — and a guard that reported them
as the probes' leak would be crying wolf. Every probe's scratch is a directory (`mkdtemp` or a `mkdir`ed root),
so nothing is lost.

## D-142 The workspace probe sampled a record the retirement was still removing (2026-09-26)

`review/dogfood/workspace.py` — the harness that walks the git-worktree lifecycle end to end (D-76) — failed
**twice in five runs** during D-141's sweeps, both times on "the worktree is gone but its record is still
there". It waited for the worktree *directory* to disappear and then checked, in the same breath, that the
member's `workspace.json` was gone; the retirement removes the directory first and the records after it, so a run
that samples the moment the directory goes reports a race that is not there. The state the harness kept proves
it: in one failing run the record was present when the probe looked and gone a minute later, with the probe's
branch merged and `git worktree list` showing only the main worktree.

**Fixed**: the probe waits for the *pair* — directory and record — inside the same 30 s it already allowed, and
fails with "the worktree was retired but its record is still there after 30 s" if the record outlives that. Five
runs after the fix passed (16–20 s each).

**Open, and not guessed at.** The other of the two failures has a shape this session could not explain. Its kept
state: the member directory holds **both** records (`workspace.json` and `worktree.json`), the worktree path is
gone, `git worktree list` shows only the main worktree — so the git-level cleanup did run — and the daemon log's
last word about that member is the *refusal* from the earlier pass ("the worktree has uncommitted, ignored or
conflicting files", which is the probe's deliberate step 2, terminating while the work is uncommitted). Nothing
was logged for the pass that removed the worktree. A member directory whose record names a worktree that no
longer exists is what `prepare` reads on a resume ("an already existing member directory wins over the project's
current state"), so if such a record really outlives the retirement it is a product-side inconsistency in the
bookkeeping rather than a probe race. That is why the probe now says so explicitly and keeps its state: the next
occurrence will show whether the record outlives the 30 s wait (a finding) or not (this race).

## D-141 The probes that take a model have one command too (2026-09-26)

D-138 gave the credential-free probes a runner; the twenty-four that take a model were still run one command at
a time — which is how the sweep that found D-140 had to be assembled, by hand. `make probe-models`
(`python3 review/dogfood/probes.py --set models`) now runs all of them, one line per probe with its duration and
what it asserts. The file was `offline.py` and it runs both sets, so it is `probes.py`; `make probe-offline`
keeps its behaviour and its minute.

The sets differ in more than membership, and both differences were measured:

* **the per-probe budget**: 300 s for the credential-free set, 900 s for the model set. `crash.py` takes about
  70 s and one `authority.py` run needed more than 400 s on a loaded host, so the 300 s budget reported a slow
  probe as a failure — the harness's own false negative, not the probe's;
* **a credential pre-check**: the model set refuses to start without `DEEPSEEK_API_KEY` (or `KIMI_API_KEY`)
  rather than failing twenty-four times for one missing key.

Three defects in the harness itself came out of running the set, and each is fixed:

1. **the guard counted daemons that were leaving**: `pkill` returns before its target is gone, so a run was red
   for a daemon that was already exiting. The guard polls for up to 15 s, and `stop_daemons` now sends SIGTERM
   and then SIGKILL for whatever is left — a daemon from a timed-out probe outlived that window once.
2. **a killed probe's findings were lost**: Python buffers stdout when it is not a terminal, so a probe the
   harness killed printed *nothing* — `authority.py`'s timed-out run had in fact emitted its own failure lines.
   The probes now run under `python3 -u` and the per-probe line is flushed, so a seven-minute run is watchable
   and a probe that is killed still explains itself.
3. **one file is in both sets**: `providers.py` (its `--self-check` half needs nothing, its session half needs
   two providers), so `--only providers.py` runs both shapes — worth knowing when reading the output.

Evidence: the full set ran in 346 s and then 1167 s, each passing 23 of 24. The failures were D-140's assertion,
`workspace.py`'s race (D-142; five runs green after the fix) and `authority.py`'s **premise**: its first turn is
supposed to be a reply that waits for the user, and the model sometimes delegates a task instead, leaving the
turn to run to its own 600 s deadline — the kept session shows thirty completed requests and a leader still
`WAITING`. That is the model's choice rather than a product failure, but the probe reports it as "the first
turn should succeed, got 124" instead of naming the premise, which is D-129's shape and is not this item's.

Ceiling: the model set is sequential and is not a benchmark; it spends real model calls; the `--provider kimi`
variants of the probes stay manual (the runner passes no provider flag); and a probe whose premise the model
declines to satisfy fails the run, as it should, with its state kept and its path printed.

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
There is now a runner for that batch, `python3 review/dogfood/probes.py --set offline` (`make probe-offline`); D-141 renamed it from `offline.py` when it gained the model set.

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

**The runner's own failure path** (added the same day). A probe that something else has to kill cannot run its
`atexit` cleanup — a signal skips it — and that is not hypothetical: `authority.py`, killed by a 420 s timeout
while the machine was saturated by this session's own load experiment, left its daemon and its scratch behind
(and the probe itself was fine: re-run on a quiet machine it passes in 26 s with the grant, the command and the
revocation all in place). `review/dogfood/probes.py` runs each probe with an explicit `--state-dir` under
its own root, reports a timeout as a failure with the probe named instead of raising out of the loop, stops
whatever serves that state root, and **keeps** the failing probe's state and prints its path — that directory
is the evidence, and D-140 is the case where it had already been removed before anyone looked. A run in which
nothing failed removes its root, so the harness leaves nothing either. Verified: a normal run reports 7 ok in
49 s with `new scratch none` and no harness root left; with the per-probe timeout lowered to 1 s all seven
report "timed out after 1s", their state is kept and named, and the daemon count stays 0.

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
  the scratch analogue of the daemon-leak guard D-111 added, with the same shape of message (**D-147**: the two
  are one implementation now, in `review/leak_guard.py`, and it names and stops what it catches).

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

**Amended by D-147**: the guard is `review/leak_guard.py` now, shared with the probe harness; the same control
prints the leaked daemon's pid and state root and stops it instead of reporting that count.

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

**2026-09-27, the witness added.** The probe no longer has to infer any of this. It starts the daemon with
`TEAMAGENTS_LOG_SURFACE=1`; the driver then writes one line per prepared request to stderr (the daemon's log) —
`driver: surface <instance> shell=yes|no tools=…` — which is a diagnostic, never persisted state, so no product
surface is added. `authority.py` prints the worker's lines before and after the grant and, when the command does
not run, says which side failed: *never offered* is the product finding, *offered and unused* is the model's
choice. The first run with it (2026-09-27, DeepSeek Flash, 33 model requests, turn 1 291 s and turn 2 338 s)
**passed**, and the log is the evidence: `shell=no` for the worker's three requests before the grant and
`shell=yes` for both after it, the worker itself reporting "a `shell` tool is now present in my toolset", and
`proof.txt` written with the expected content — the worker's own lines are kept in
`review/tmp/d143-surface-witness/` (a copy: the harness's root is episode-local, D-156). So the surface
follows the grant in this shape, and the failing run recorded above is now either a model that did not use
an offered tool or an intermittent path that the next failure will name instead of hiding behind the
model's account of its own tool list.

**2026-09-27, an exception reproduced and narrowed.** A `make probe-models`-style run of
`python3 review/dogfood/authority.py` failed *this* way (kept session under the harness root of that run):
the user grant `shell@workspace` for `worker_shell_probe` was issued and unrevoked, the worker's `workspace_ref`
was the shared workspace and its stored profile still carried the `shell` schema, yet both of its following
task-driven turns *reported* thirteen tools without `shell` (`ls … skill, wait, finish, read_history`) and
answered `blocked`; the probe's own turn timed out at 600 s and the harness's 1800 s budget killed the probe. (The kept session was read while it existed and then deleted by the pid-named-root collision of D-156; the facts above are what reading it recorded.)
The probe's premise — "a live grant means the tool was offered" — is exactly what the worker's transcript
contradicts, and the harness test for the same shape
(`v2_supervisor::a_task_driven_worker_turn_sees_a_live_user_grant`, added then) **passes**: the supervisor path
offers `shell` on a task-driven turn prepared after a user grant. So the open question is now narrow: either the
daemon-run session prepared those requests on another path, or the worker misreported its own tool list — its
second turn's `est_prompt_tokens` (2128, against 1411 for the first) leans towards the schema having been there,
and that estimate is coarse. Settling it needs the per-request offer to be observable (the persisted surface
listed below as needing the user's word) or one instrumented run; until then the probe keeps printing what it
knows (`grant=live|ABSENT`, the worker's attempts, `task=`).

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

Scenario 1's runtime check is now `exit 1` (D-146): the `test -f never-written` configured here was not
impossible after all — the model can write that file — and a later run did, settling the goal `SUCCEEDED`.

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
`<state root>/daemon.sock` was never created); `tui::cli_flags::the_tui_refuses_the_flags_it_cannot_honour` (renamed in D-180, which added `--engine` to the set)
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
  graph may be arbitrary, replacing the earlier (v1) D-33's member-to-member restriction. The shared project
  directory is
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
