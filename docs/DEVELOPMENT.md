# Development and maintenance

The project keeps three Rust crates: `core`, `engine` and `tui`. The product constraints live in the
[design baseline](DESIGN.md), confirmed decisions in [DECISIONS](DECISIONS.md) and the real-service
acceptance boundary in [ACCEPTANCE](ACCEPTANCE.md).

## Environment and the single entry point

Work from the repository root. `rust-toolchain.toml` pins the Rust version, Clippy and rustfmt; the local
machine and both GitHub workflows read the same pin. After installing Rust through rustup, the first Cargo
run prepares the toolchain. The Linux isolation checks also need a working bubblewrap, and the real-terminal
check needs Python 3.

```bash
make check CARGO_FLAGS=--locked   # first run may download dependencies, keeping the lock files
make check                        # afterwards: --offline --locked by default
make check-nobwrap                # the same gate with the GitHub runner's condition: no bwrap in PATH (D-113)
make check-broken-sandbox         # ... and with a bwrap that cannot start a sandbox (Ubuntu 24.04 default, D-114)
make fmt                          # apply the shared formatting
make build                        # build the CLI and the TUI into the usual target paths
make pty                          # real-terminal check with an isolated config and no model credentials
```

`make test` wraps the three crate suites in `review/leak_guard.py` (snapshot before, audit after): the two ways
a run leaks — a session daemon (D-111) and a scratch directory (D-131) — are reported as a *difference*, with
the leaked daemon's pid and state root named and the daemon stopped, and the run fails (D-147). The tests hold
up their end with two RAII guards in `engine/tests/cli.rs`: `Daemon` stops a daemon the test started, on every
exit path including a panic (D-160), and `Scratch` removes the test's own temp tree the same way (D-175) — a
test that panics in a CI condition used to skip its final `remove_dir_all`, so one failure reported as two and
left state roots behind. After the suites, `review/test_counts.py` compares their exact sizes (asked of
cargo's test list) with the numbers `docs/ACCEPTANCE.md` states in its baseline line, so the ledger's
headline cannot go stale (D-178). `make check` runs, in order: formatting (`make fmt-check`), Clippy on all
targets with warnings as errors, the three crates' tests and repository hygiene. Hygiene is a set of small
audits — each one a claim the tree makes about itself, each with the decision that built it, and
`review/hygiene_catalogue.py` fails when a script the build runs is missing from this list (D-184):

* **the documents**: `review/decisions_log.py` keeps `docs/DECISIONS.md`'s shape — one heading per entry,
  newest-first, each with a body (D-107); `review/decision_queue.py` keeps the user's decision queue
  complete — an entry that asks for the user's word must appear in `docs/ACCEPTANCE.md`'s known gaps or
  `docs/PRODUCT-COMPARISON.md` §2, or name the later decision that closed it (D-195); `review/citations.py` resolves every backticked citation in tracked
  markdown, a qualified name or a repository path, unless the line records it as removed (D-110);
  `review/decision_citations.py` resolves every `D-<n>` to a heading or to one of the earlier rules the index
  table keeps in force (D-170); `review/readme_zh.py` holds `README.zh-CN.md` to `README.md` on the heading
  skeleton, the in-repository links and the CLI surface (D-134); `review/doc_flags.py` holds the three user-facing documents (the README, the user guide and the install guide) and the parsers against the CLI's own help text — a documented flag must exist, a parsed flag must be advertised, and a line marked as history is a note (D-135/D-136/D-190); `review/exec_report.py` does the same for the five `--json` reports (D-154) and
  `review/tui_keys.py` for the TUI's keys (D-157); `review/requirement_trace.py` keeps the baseline's Q-rows
  and `docs/ACCEPTANCE.md`'s rows in step (D-137).
* **the generated references**: `review/event_catalogue.py` (`docs/EVENTS.md` vs the emitted events, D-125),
  `review/protocol_catalogue.py` (`docs/PROTOCOL.md` vs the daemon's dispatchers and the row-field table,
  D-126/D-173), `review/tool_catalogue.py` (`docs/TOOLS.md` vs the tool schemas, D-127) and
  `review/config_reference.py` (`docs/CONFIG.md` vs the config structs, D-128).
* **what the code does with what it is given**: `review/flag_fields.py` fails a flag parsed into an `Args`
  field no code reads (the `--engine` shape, D-180/D-181); `review/command_params.py` fails a command payload
  field the command layer never reads (D-124); `review/dead_code.py` lists the public items the product's own
  code never calls (D-78/D-86, in hygiene since D-130); `review/env_knobs.py` keeps the `TEAMAGENTS_*` table
  below equal to the code's reads; `review/project_config_claim.py` checks the one fact every document states —
  the project config is not read (D-133); `review/silent_skips.py` fails a test that returns from a capability
  guard without saying why (D-121).
* **the config surface, both directions**: `review/config_keys.py` reports a config field whose only readers
  are the loader, the validator, the doctor surface and the argv parser — a key this build accepts and never
  serves (the detector behind D-75 and D-102, in hygiene since D-193) — and fails when the shipped examples or
  the `toml` blocks of the three user-facing documents name a key the loader does not accept, or one it refuses.
* **the build and its evidence**: `review/build_references.py` fails a script a `Makefile` target or a
  workflow runs that does not exist or that git does not carry — how a new audit stays untracked through a
  `git commit -a` (D-179), and since D-196 the same audit keeps the Makefile's `.PHONY` targets and its `make help` text naming each other (the help is the default goal); `review/hygiene_catalogue.py` fails a script the build runs that this page does not name (D-184) **and** a root-level `review/*.py` that no target runs unless it is listed in that audit's `HAND_RUN` with its reason (D-194: `review/config_keys.py`, the detector behind four findings, was in that position until D-193); `review/eval_manifests.py` holds the frozen evaluation manifests against the tree
  (D-145); `review/eval_surface.py` holds the evaluation's model-visible surface against those manifests, the
  harness's history and the recorded trials (D-182); `review/dogfood/probes.py --self-check` checks the probe
  harness's own rules (its selection, budgets, the stray guard and the no-kill-by-pattern rule);
  `review/verification_catalogue.py` holds `verification/tla`'s configurations and modules against the
  `verify-model*` targets that drive them, and the report's counts — the configurations, the negative controls
  and the quoted Kani harness count — against the lists and the proofs in the tree (D-185).
* **the shell, before any of that**: it rejects non-English characters (the two documented exceptions), tracked
  compile caches, Python caches and SQLite temporaries, and checks the syntax of `install.sh`.

`make language-check` runs the language rule on its own: it scans the tracked tree for CJK characters and
excludes exactly the two documented exceptions (`README.zh-CN.md` and the frozen evaluation material under
`review/eval`), so a stray Chinese comment fails the gate instead of being noticed in review. Clippy warnings
are errors. CI uses the same make targets and may only download the locked dependencies; the release workflow
uses the same Rust version.
A green Cargo run can include tests that returned early for a missing dependency, so it never substitutes for
real isolation or real-model acceptance.

### Evaluation and probe entry points

Beyond the product there are a few standalone entry points: the failure/overhead probes, the A/B/C
evaluation drivers and the real-terminal driver. None of them is part of `make check`; when re-running them,
pick a **fresh evidence directory** and record results and limits in a dated report:

```bash
# dogfooding: the built CLI on this repository's own fixtures, with each fixture's
# checks.txt as a user-defined completion check, and the artifact verified by hand
python3 review/dogfood/run.py --task edit-integrity   # needs the model credential
# the authority surface end to end (spawn a worker, fail without the grant, `authority grant`,
# retry, `authority revoke`) — the probe that found D-62
python3 review/dogfood/authority.py
# failure probes: SQLite/artifact atomic boundaries, runner and daemon crash recovery, storage-full stop, I/O cancel
cargo build --offline --locked --manifest-path engine/Cargo.toml --example probe
cargo build --offline --locked --manifest-path tui/Cargo.toml --example probe
engine/target/debug/examples/probe suite review/tmp/probe-new
python3 tui/scripts/pty_probe.py            # drives the same protocol in a real terminal

# evaluation drivers (real models; A = direct reference loop, B = persistent single instance, C = B + collaboration)
cargo build --offline --manifest-path engine/Cargo.toml --example eval_group_a
engine/target/debug/examples/eval_group_a --task "..." --workdir /tmp/t --trace /tmp/t-trace
cargo build --offline --manifest-path engine/Cargo.toml --example eval_group_b   # needs engine/target/debug/teamagents
engine/target/debug/examples/eval_group_b --task "..." --workdir /tmp/t --trace /tmp/t-trace --full-auto
engine/target/debug/examples/eval_groups_abc --group A --task-file t.md --workdir /tmp/t --trace /tmp/t-trace --state /tmp/t/state

# load and acceptance probes
engine/target/debug/examples/load_probe DIR [--steps N] [--payload BYTES]
engine/target/debug/examples/accept_probe --evidence DIR --workspace DIR --lead KEY --worker KEY

# the host's leftover command runners (D-189): a read-only census with the classes that decide what may be
# stopped. Run it *outside* any sandbox — a sandboxed shell sees only its own PID namespace and reports zero.
python3 review/host_cleanup.py [--json | --class settled-journal --pids]
```

Deterministic coverage of fault injection and multi-instance behaviour lives in the test files:
`cargo test --manifest-path engine/Cargo.toml --test v2_driver` (crash reuse, disk full, cancellation,
required checks), `--test jobs_runner` (runner/daemon crashes, duplicate GO), `--test v2_supervisor`
(multi-instance scheduling and retirement) and `--test v2_daemon` (handshake, watermark resume).

Local development still uses Cargo directly for a shorter feedback loop (the list below is every test entry
point):

```bash
# core: control plane, kernel, persistence
cargo test --offline --locked --manifest-path core/Cargo.toml --lib   # control-plane unit tests (most A01-A36 citations)
cargo test --offline --locked --manifest-path core/Cargo.toml --test v2_invariants
cargo test --offline --locked --manifest-path core/Cargo.toml --test kernel_properties
# engine: phase machine, supervisor, daemon, jobs, fake services, CLI
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_driver
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_supervisor
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_daemon
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_mcp
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_spawn_failure
cargo test --offline --locked --manifest-path engine/Cargo.toml --test jobs_runner
cargo test --offline --locked --manifest-path engine/Cargo.toml --test providers_fake
cargo test --offline --locked --manifest-path engine/Cargo.toml --test providers_stall
cargo test --offline --locked --manifest-path engine/Cargo.toml --test reference_loop
cargo test --offline --locked --manifest-path engine/Cargo.toml --test cli
cargo test --offline --locked --manifest-path engine/Cargo.toml --test install
# tui: conversation state and rendering
cargo test --offline --locked --manifest-path tui/Cargo.toml --test v2app_tests
```

## Environment knobs the code reads

None of these *configures* behaviour — a setting belongs in `config.toml` (`docs/CONFIG.md`); they locate a
binary, turn on a diagnostic, or exist for tests. They are listed because a knob nobody can find is a knob
nobody uses, and several are what the probes depend on.

| Variable | Who sets it | What it does |
|---|---|---|
| `TEAMAGENTS_LOG_SURFACE=1` | an operator diagnosing a member's offer, and `review/dogfood/authority.py` | the driver writes one line per prepared request into stderr (the daemon's log): `driver: surface <instance> shell=yes\|no tools=…` — the witness that separates "the tool was never offered" from "the model ignored it" (D-143) |
| `TEAMAGENTS_JOB_IDLE_TICK_MS` | tests | how often an *idle* job runner re-checks (default 30 s): a test cannot wait 30 s for a rule about waiting (D-153) |
| `TEAMAGENTS_JOB_TEST_HOOKS` | the runner's own tests | compiles-in fault injection for the job runner, disabled unless the parent opts in; never read from a job file or a model (A31) |
| `TEAMAGENTS_RUNNER_BIN` | integration tests | the runner image, so the test binary can serve as the runner (production uses the same `teamagents` binary) |
| `TEAMAGENTS_TUI` | the CLI, when the TUI is not a sibling of it | where `teamagents` finds `teamagents-tui`; its own error message names this variable (`engine/src/main.rs`) |
| `TEAMAGENTS_BIN_DIR` | `install.sh` | where the installer puts the binaries; documented for users in `docs/INSTALL.md` |

## Formal verification (TLA+ / Kani)

`verification/` holds formal material that is tied to the implementation: `tla/V2*.tla` with `MC*.cfg` are the
TLA+ specs of the control plane, artifacts, waits, tasks, compression, the daemon protocol, the required
check rounds, the authority layer and the user's authority surface; `kani/` is a proof crate that compiles
`core/src/kernel/types.rs` directly. The results, evidence and the **unproven list** are in the
[verification report](../verification/REPORT.md); the mapping from property to code to acceptance item is in
the [verification guide](../verification/README.md).

```bash
make verify-tools       # download and verify the pinned tla2tools.jar (TLC v1.7.1, fixed SHA-256)
make verify-model       # small control-plane configuration
make verify-model-all   # small configurations for all nine modules (~2 minutes)
make verify-model-counterexamples  # the authority surface's negative controls: each must be *refuted*
make verify-model-wide  # wide control-plane configuration (hundreds of millions of states, slow)
make verify-kani        # Kani proofs for the paging arithmetic (needs the Kani toolchain)
cargo test --offline --locked --manifest-path core/Cargo.toml --test v2_invariants
```

These targets are not part of `make check` (they need Java/Kani and the first run downloads TLC). TLC's
`verification/tla/states/` and Kani's `target/` are regenerable intermediates that `.gitignore` excludes.
The models cover protocol-level properties only: they are not a refinement proof, liveness depends on weak
fairness assumptions and every enumeration is bounded. When the command set, the phase machine or the
paging/capping logic changes, update the specs and `v2_invariants` and re-run the affected targets.

## Where a change belongs

| Change | Location and constraints | Preferred regression |
|---|---|---|
| Team actions, tasks, grants, scheduling | `core/src/v2/control.rs`, the single-transaction `Control::submit` path; identity, operation ids and permission revisions are filled in by the control plane, never taken from a model or client field | `core` library tests (`core/src/v2/control.rs`), `core/tests/v2_invariants.rs`, `engine/tests/v2_supervisor.rs` |
| Persistence and transaction boundaries | `core/src/v2/store.rs` (one database per session, WAL plus explicit `synchronous=FULL`); in-process callers serialize through the bounded single-writer worker in `engine/src/v2/storage.rs` | `core` library tests, `engine/tests/v2_driver.rs`, `core/tests/v2_invariants.rs` |
| Turn phase machine and tool execution | `engine/src/v2/driver.rs`: model and tool waits stay outside transactions and every transition goes through `Control::submit`. File/Shell/web tools live in `engine/src/tools.rs`, Shell commands run in the runner process of `engine/src/jobs`, and user hooks are `engine/src/hooks.rs` (`pre_tool` veto plus `notify` events) | `engine/tests/v2_driver.rs`, `engine/tests/jobs_runner.rs`, `engine/tests/providers_fake.rs`, `providers_stall.rs` |
| Multi-instance collaboration | `engine/src/v2/supervisor.rs`: one phase machine per ACTIVE instance; the authorization and dispatch linearization point for `spawn`/`delegate`/`send`/`wait` is `core/src/v2/control.rs` | `engine/tests/v2_supervisor.rs`, `core/tests/v2_invariants.rs` |
| Session daemon and clients | `engine/src/v2/daemon.rs` (one Unix-socket JSON-lines service per state root), `engine/src/v2/exec.rs` (headless client), `tui/src/daemon_client.rs` (resumes events from the last watermark) | `engine/tests/v2_daemon.rs`, `engine/tests/cli.rs`, `make pty` |
| Information permissions and shared space | visibility and delivery decisions in `core/src/v2/control.rs` plus the context views in `core/src/kernel/*`; `audience` visibility is not `push` delivery | `core` library tests (visibility, delivery, references), `core/tests/v2_invariants.rs` |
| MCP, Skills and tool bindings | `engine/src/bound.rs` (binding is the authorization), `engine/src/mcp.rs` (stdio and streamable HTTP) | `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs` |
| Workspace policies | `engine/src/workspace.rs` (shared / isolated / git worktree). The `workspace` argument of `spawn` is resolved by `driver::prepare_spawn_workspace`, the record is written to `<instances_dir>/<id>/workspace.json`, and retirement happens in the supervisor's terminate path | `engine/src/workspace.rs` unit tests, `engine/tests/v2_driver.rs::spawn_resolves_the_requested_workspace_policy`, `engine/tests/v2_supervisor.rs::terminating_an_instance_retires_its_workspace` |
| Providers | `engine/src/providers/*`: exactly one transport attempt and failure classification only, retries belong to the runtime; config and catalog live in `engine/src/config.rs` (user catalog parsing, `[hooks]`/`[retention]` validation) | `engine/tests/providers_fake.rs`, `engine/tests/providers_stall.rs`, `engine/src/config.rs` unit tests |
| Model calls and the kernel | `core/src/kernel/*` (I/O-free request/response/observation conversion), `engine/src/reference.rs` (the group A direct reference loop) | `core/tests/kernel_properties.rs`, `engine/tests/reference_loop.rs` |
| Conversation UI and the real terminal | `tui/src/v2app.rs` (state and keys), `tui/src/v2ui.rs` (rendering; `geometry()` also feeds mouse hit-testing), `tui/src/wrap.rs` | `tui/tests/v2app_tests.rs`, `make pty` |
| Install, self-check, release | `install.sh`, `init`/`doctor` in `engine/src/cli.rs`, `.github/workflows/release.yml` | `engine/tests/install.rs`, `engine/tests/cli.rs`, release-archive smoke test |

Rust paths in the table are relative to each crate's `src/`. The TUI reaches the engine only through the
daemon socket (`tui/src/daemon_client.rs`): it never reads the database and never executes anything in its
own process. Callbacks and queue types are named in their own module so complex signatures are not copied
across files. Extract a module around an independent responsibility and a real change rate; do not create a
framework used in one place just to silence a lint.

## Stable regression tests

Integration tests prefer isolating `XDG_CONFIG_HOME`/`XDG_STATE_HOME` through a child process
(`Command::env`, see `engine/tests/cli.rs`). When a test really must change the process environment, it
touches only its own variables and restores them inside the same test; engine library tests keep using the
existing `crate::env_lock()` convention. Test projects for production sessions must be separate from the
config and state directories — use `project/`, `config/` and `state/` under one temporary root, and never
share a temporary root containing XDG state (or `/tmp`) as the working root.

Fake services read the request before answering, so no response can precede the pending request. Concurrency
assertions prefer barriers or channels, and the environment is torn down only after every thread and child
process has been closed and joined; never rely on test-name ordering or on the machine's own user config.

Coverage by layer: `v2_driver` owns result reuse after a crash (never re-executing side effects), the
disk-full stop and recovery, cancellation and required checks; `jobs_runner` owns runner/daemon crashes,
duplicate GO and services surviving a daemon exit; `v2_supervisor` owns multi-instance scheduling;
`v2_daemon` owns the handshake, watermark resume and refusing a second daemon; `v2_mcp` owns binding,
approval and cancellation; `providers_fake`/`providers_stall` own truncated streams, connection loss and
retry; `v2_invariants` maps the formal invariants back onto the current command set. All of them use local
fixtures and fake services; evidence and boundaries are in [ACCEPTANCE](ACCEPTANCE.md) and they never count
as real-model or real-provider acceptance.

## Code review and evidence

- The shared formatting lives in the root `rustfmt.toml`; submit large formatting sweeps separately from
  behaviour changes so the real logic stays reviewable.
- Never disable a lint crate-wide. An existing wide interface that must stay can carry a function-level
  `#[expect(..., reason = "...")]`; an expectation that lost its trigger also fails the strict check.
- File opens state whether they keep, append or truncate, and lock files keep their inode and content — they
  are never turned into truncating opens to silence a lint.
- New logic is verified for success, failure and recovery; a pure move or reformat reuses the existing
  regression instead of adding a mirrored test.
- `review/tmp/` is the ignored probe and scratch area. Durable conclusions go into a dated `review/*.md`,
  evaluation evidence into `review/eval/r2-p6/runs/`; Python caches, temporary databases, nested Git
  repositories and full copies of old sources are never committed.

## Dependencies, toolchain and releases

Each crate commits its own `Cargo.lock`, and both the regular checks and the release build use `--locked`.
Before adding a dependency, check whether the standard library or an existing dependency already covers the
need; when upgrading one, update only the affected lock files and re-run the interface checks plus
`make check`. Upgrading Rust means editing the version in `rust-toolchain.toml` and re-running `make fmt`,
`make check` and `make pty`; CI and the release workflow pick the new version up automatically.

Before a release, bump the three crates' versions and their lock files, update the release notes, pass CI and
then push the `vX.Y.Z` tag. Afterwards verify the SHA-256 from the public URL and check the install,
`init` keeping the existing config and a real TUI start. Real-model evaluation needs explicit credentials and
the model's native context, recorded separately; a maintenance regression never claims to widen provider
compatibility.

## Fixed tasks and grading

The task set and runner live in `review/eval/r2-p6/`: each task `tasks/<id>/` provides `prompt.md`,
`checks.txt` and an optional `fixture/`; `run.py` gives every trial a fresh working directory and state
directory, grades it inside **that same directory** from `checks.txt` once the trial ends, and writes the
result to `runs/<date>/results.jsonl`. Group definitions, frozen parameters and limits are in the
[evaluation guide](../review/eval/README.md).

- Grading runs inside the trial directory only and never reads the current work tree; task inputs and grading
  scripts are frozen before a run (`manifest*.json` records the hashes).
- Build output of large tasks can reach several GB: check the filesystems with `df -h /tmp .` first, put the
  output directory in the ignored `review/tmp/` or on a separate disk, and point `TMPDIR` at the same disk.
- Existing evidence is never cleaned automatically; during a run the inputs, grading and candidates stay
  untouched, and a historical failure is never re-recorded as a success because storage moved.
- When comparing against a competitor CLI that can read files outside the workspace, putting the tests far
  away is not enough: mount only the public inputs in an isolated filesystem view and use a
  model-free probe to confirm the hidden material is unreadable while the workspace stays writable and the
  other sandbox is still active. If the model already read hidden material, stop, keep the polluted trace and
  re-run with fresh inputs; never continue from the polluted context.

## Real-model verification

Real-model evidence comes from three explicit entry points, none of which is part of `make check` (they need
credentials and a native context configuration):

```bash
# A/B/C comparison over the fixed task set (pre-registration and frozen parameters in review/eval/r2-p6/design.md)
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<new date>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<new date>
# one headless turn: starts the daemon when needed and reports the outcome (--json for a machine-readable summary)
engine/target/debug/teamagents exec --json --timeout 180 "1+1=?"
# direct reference loop (group A entry, same kernel/tools/config)
engine/target/debug/examples/eval_group_a --task "..." --workdir /tmp/t --trace /tmp/t-trace
# the dogfood probes: the built CLI, real daemons and the real TUI, driven end to end
make probe-offline                       # the probes that need no model and no credential, ~1 min
make probe-models                        # the probes that take a model, one after another (~7 min)
python3 review/dogfood/checks.py --state-dir /tmp/ta-checks   # one model probe at a time
```

- **The probes** are in `review/dogfood/` (`review/dogfood/README.md` describes each one and what it asserts).
  `make probe-offline` runs the ones that need no model and no credential; `make probe-models` runs the rest,
  one after another (`python3 review/dogfood/probes.py --list` prints both sets and why each probe is in one). Both fail if the run leaves a daemon or a scratch directory behind
  (D-111, D-131; the same guard as `make test`, D-147) and keep the state of a probe that failed (D-138).
- Always use the model's native context length and record the value and its source in the report (D-36);
  DeepSeek Flash uses the user-confirmed 1M.
- Every trial uses a fresh working and state directory; results go to `runs/<date>/results.jsonl` and each
  trial's session database and artifacts stay in `runs/<date>/{state,work}/`. Compile caches and SQLite
  temporaries of a trial are **never committed**: `.gitignore` excludes `target/` and `*.sqlite-wal|shm`,
  and `make hygiene` rejects them.
- Conclusions follow the pre-registered criteria only; with too few samples, an interval containing 0 or too
  much variance, write "not confirmed" — never "equivalent" and never a claimed gain.

Operational notes for real-model runs (tasks can park in `BLOCKED`, approvals expire with the turn, heavy
tasks should write early) are in the repository's [AGENTS.md](../AGENTS.md).
