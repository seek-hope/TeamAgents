# Development and maintenance

TeamAgents has three Rust crates: `core`, `engine` and `tui`. Product requirements live in
[DESIGN.md](DESIGN.md), confirmed decisions in [DECISIONS.md](DECISIONS.md), and acceptance evidence in
[ACCEPTANCE.md](ACCEPTANCE.md).

Generated references are [EVENTS.md](EVENTS.md), [PROTOCOL.md](PROTOCOL.md), [TOOLS.md](TOOLS.md) and
[CONFIG.md](CONFIG.md). Edit their source schemas or dispatchers, then regenerate them with the commands in
this guide; do not patch generated tables to describe behavior the code does not have.

<!-- maps: docs/CONFIG.md reader=user -->
<!-- maps: docs/EVENTS.md reader=developer -->
<!-- maps: docs/PROTOCOL.md reader=developer -->
<!-- maps: docs/TOOLS.md reader=developer -->
<!-- maps: docs/RUST-SANDBOX.md reader=evidence -->

[RUST-SANDBOX.md](RUST-SANDBOX.md) is a retired design note kept for cited historical evidence. It does not
describe current execution or authorize reintroducing a backend.

## Environment and the single entry point

Work from the repository root. `rust-toolchain.toml` pins Rust, Clippy and rustfmt for local development and
both GitHub workflows. The first Cargo run may download them. Python 3 is required for repository audits and
PTY checks. Shell and local MCP tests run on the host.

```bash
make check CARGO_FLAGS=--locked   # first run; allows locked dependency downloads
make check                        # later runs; offline and locked by default
make fmt                          # apply rustfmt
make fmt-check                    # check formatting without rewriting files
make build                        # build CLI and TUI
make pty                          # terminal-profile matrix in isolated ptys
```

`make check` runs formatting, Clippy with `-D warnings`, the three crate test suites and repository hygiene.
There is no root Cargo workspace; use `make` or explicit crate manifests.

`make test` wraps the suites with `review/leak_guard.py`. It compares daemon, runner and scratch-directory
inventories before and after the run, stops only processes it identifies by pid, and preserves the state root
for diagnosis. `review/test_counts.py` then compares the suite sizes with the baseline in ACCEPTANCE and the
corresponding statements in AGENTS.md and `.github/release-notes.md`.

Run leak-guarded suites serially with live probes; their process and scratch inventories can otherwise
interfere. Build output needs writable space; use `CARGO_TARGET_DIR` for a read-only source mount. Keep the
real Rust and Cargo homes available instead of hiding them with an arbitrary `HOME` or `PATH` change.

### Hygiene checks

`make hygiene` checks language, tracked artifacts, script syntax and the following source/document contracts.
`review/hygiene_catalogue.py` requires every script invoked by `make hygiene` or `make test` to be named here,
and requires every root-level audit either to run in a target or to have a recorded hand-run reason.

| Area | Checks |
|---|---|
| Build and review inventory | `review/build_references.py`, `review/hygiene_catalogue.py`, `review/doc_inventory.py` |
| Decisions, requirements and citations | `review/decisions_log.py`, `review/decision_queue.py`, `review/decision_citations.py`, `review/citations.py`, `review/requirement_trace.py` |
| User and interface docs | `review/project_config_claim.py`, `review/release_artifact.py`, `review/readme_zh.py`, `review/doc_flags.py`, `review/exec_report.py`, `review/tui_keys.py`, `review/env_knobs.py`, `review/markdown_tables.py` |
| Configuration and control surface | `review/config_keys.py`, `review/flag_fields.py`, `review/command_params.py`, `review/config_reference.py` |
| Generated tool/protocol references | `review/event_catalogue.py`, `review/protocol_catalogue.py`, `review/tool_catalogue.py` |
| Source and test quality | `review/dead_code.py`, `review/silent_skips.py`, `review/leak_guard.py`, `review/test_counts.py` |
| Evaluation and formal material | `review/eval_manifests.py`, `review/eval_surface.py`, `review/eval/r2-p6/anatomy.py`, `review/verification_catalogue.py` |
| Probe registry | `review/dogfood/probes.py` |

The audits catch structural drift, not every incorrect claim. Review current behavior and evidence dates
manually. Do not suppress a source-side validation or mark a current result as history merely to make a check
pass.

`make language-check` rejects non-English characters outside `README.zh-CN.md` and the frozen evaluation
material. Commit messages are English too. `git diff --check` and the help recipe run as part of hygiene, so
whitespace errors and shell-quoting errors in help text fail the gate.

### Focused tests

Use Cargo directly for a shorter loop. These are common entry points, not a replacement for the full suites:

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --lib
cargo test --offline --locked --manifest-path core/Cargo.toml --test v2_invariants
cargo test --offline --locked --manifest-path core/Cargo.toml --test kernel_properties

cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_driver
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_supervisor
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_daemon
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_mcp
cargo test --offline --locked --manifest-path engine/Cargo.toml --test jobs_runner
cargo test --offline --locked --manifest-path engine/Cargo.toml --test providers_fake
cargo test --offline --locked --manifest-path engine/Cargo.toml --test cli
cargo test --offline --locked --manifest-path engine/Cargo.toml --test install

cargo test --offline --locked --manifest-path tui/Cargo.toml --test v2app_tests
cargo test --offline --locked --manifest-path tui/Cargo.toml --test conversation
cargo test --offline --locked --manifest-path tui/Cargo.toml --test graphics
cargo test --offline --locked --manifest-path tui/Cargo.toml --test parity
cargo test --offline --locked --manifest-path tui/Cargo.toml --test sessions
```

### Evaluation and probe entry points

Standalone probes and evaluation drivers are not part of `make check`. Use a fresh evidence directory and
record the source, command, result and limitations in a dated report.

```bash
# Repository fixtures and authority workflows; model credentials depend on the probe.
python3 review/dogfood/run.py --task edit-integrity
python3 review/dogfood/authority.py

# Failure and overhead probes.
cargo build --offline --locked --manifest-path engine/Cargo.toml --example probe
cargo build --offline --locked --manifest-path tui/Cargo.toml --example probe
engine/target/debug/examples/probe suite review/tmp/probe-new
python3 tui/scripts/pty_probe.py

# A = direct reference loop; B = persistent instance; C = B plus collaboration.
cargo build --offline --locked --manifest-path engine/Cargo.toml --example eval_group_a
engine/target/debug/examples/eval_group_a --task "..." --workdir /tmp/t --trace /tmp/t-trace
cargo build --offline --locked --manifest-path engine/Cargo.toml --example eval_group_b
engine/target/debug/examples/eval_group_b --task "..." --workdir /tmp/t --trace /tmp/t-trace --full-auto
cargo build --offline --locked --manifest-path engine/Cargo.toml --example eval_groups_abc
engine/target/debug/examples/eval_groups_abc --group A --task-file t.md --workdir /tmp/t --trace /tmp/t-trace --state /tmp/t/state

# Load and acceptance probes.
engine/target/debug/examples/load_probe DIR [--steps N] [--payload BYTES]
engine/target/debug/examples/accept_probe --evidence DIR --workspace DIR --lead KEY --worker KEY

# Real CodeGraph discovery + codemode query; existing index only, no model.
cargo run --offline --locked --manifest-path engine/Cargo.toml --example codegraph_probe -- .

# Read-only host process census. Run on the host so container PID namespaces cannot hide processes.
python3 review/host_cleanup.py [--json | --class settled-journal --pids]
```

## Environment knobs the code reads

These variables locate binaries, select diagnostics or support tests. Product behavior belongs in
`config.toml`; see [CONFIG.md](CONFIG.md).

| Variable | Setter | Purpose |
|---|---|---|
| `TEAMAGENTS_LOG_SURFACE=1` | Operator | Log each prepared model request's offered tools to the daemon log |
| `TEAMAGENTS_JOB_IDLE_TICK_MS` | Tests | Idle runner polling cadence, default 30 s |
| `TEAMAGENTS_AUTOMATION_TICK_MS` | Tests | Automation scheduler cadence, default 30 s |
| `TEAMAGENTS_JOB_TEST_HOOKS` | Runner tests | Enable explicit fault injection; never read from model input or job files |
| `TEAMAGENTS_RUNNER_BIN` | Integration tests | Runner executable used by the test harness |
| `TEAMAGENTS_GRAPHICS` | Users and PTY probes | `auto`, `kitty`, `blocks` or `off` |
| `TEAMAGENTS_SESSION_CLI` | CLI launcher | Engine executable used by the session browser |
| `TEAMAGENTS_TUI` | CLI or caller | TUI path when it is not beside `teamagents` |
| `TEAMAGENTS_BIN_DIR` | Installer | Binary destination |
| `TEAMAGENTS_INSTALL_ROOT` | Updater/operator | Versioned installation root, default `~/.local/lib/teamagents` |
| `TEAMAGENTS_UPDATER_BIN` | Update tests/operator | Updater executable override |
| `TEAMAGENTS_UPDATE_CRASH_AT` | Update tests | Explicit crash-injection point |
| `TEAMAGENTS_DISPLAY_WORKER_TESTS` | TUI tests | Parent-controlled display-worker fault injection |

Display workers are an internal rendering mode of `teamagents-tui`, not a configurable executable. They clear
the environment, close inherited file descriptors and apply resource limits before parsing. Build the TUI
before focused tests that launch the helper. Source remains readable when rendering fails.

## Formal verification (TLA+ / Kani)

`verification/tla/` contains the TLA+ modules and configurations. `verification/kani/` compiles selected
kernel and viewport code directly. The [verification guide](../verification/README.md) maps properties to
code and acceptance items; the [verification report](../verification/REPORT.md) records results and unproven
areas.

```bash
make verify-tools       # download and verify the pinned TLC v1.7.1 jar
make verify-model       # small control-plane configuration
make verify-model-all   # small configurations for all modules
make verify-model-counterexamples
make verify-model-wide  # large, slow configuration
make verify-kani        # requires the Kani toolchain
make benchmark-receipts JOBS=<dir> [BENCH=swebench|tau3|tb]
cargo test --offline --locked --manifest-path core/Cargo.toml --test v2_invariants
```

These targets are outside `make check`; they need Java or Kani, and the first TLC run downloads the pinned
tool. Each TLC invocation uses a separate temporary meta directory under `review/tmp/tla/` and removes it on
return. Regenerable intermediates are excluded by `.gitignore`.

Formal models cover bounded protocol properties, not refinement of the whole implementation. Liveness depends
on its stated fairness assumptions. Update affected models and invariants when commands, phases or arithmetic
change, and record a fresh run instead of inheriting an older report's result.

## Where a change belongs

| Change | Location and invariant | Preferred regression |
|---|---|---|
| Team actions, tasks and grants | `core/src/v2/control.rs`; all writes through `Control::submit`, with control-plane identity and revisions | Core tests, `core/tests/v2_invariants.rs`, `engine/tests/v2_supervisor.rs` |
| Persistence | `core/src/v2/store.rs`; one database per session, WAL and `synchronous=FULL`; writer serialization in `engine/src/v2/storage.rs` | Core tests, `engine/tests/v2_driver.rs` |
| Turn and tool execution | `engine/src/v2/driver.rs`; model/tool waits outside transactions. Native tools in `engine/src/tools.rs`, jobs in `engine/src/jobs` | `engine/tests/v2_driver.rs`, `engine/tests/jobs_runner.rs` |
| Multi-instance coordination | `engine/src/v2/supervisor.rs`; one driver per ACTIVE instance | `engine/tests/v2_supervisor.rs` |
| Daemon and clients | `engine/src/v2/daemon.rs`, `engine/src/v2/exec.rs`, `tui/src/daemon_client.rs` | `engine/tests/v2_daemon.rs`, `engine/tests/cli.rs`, `make pty` |
| Information visibility | `core/src/v2/control.rs` and `core/src/kernel/*`; visibility is not delivery | Core tests and invariants |
| MCP, Skills and bindings | `engine/src/bound.rs`, `engine/src/mcp.rs`; local services execute on the host | `engine/tests/v2_mcp.rs`, `engine/tests/v2_spawn_failure.rs` |
| Workspace policies | `engine/src/workspace.rs`; spawn resolution in the driver, retirement in the supervisor | Workspace tests and supervisor integration tests |
| Providers and configuration | `engine/src/providers/*`, `engine/src/config.rs`; providers classify attempts, runtime owns retries | `engine/tests/providers_fake.rs`, `engine/tests/providers_stall.rs`, config tests |
| I/O-free kernel and reference loop | `core/src/kernel/*`, `engine/src/reference.rs`; the reference loop is evaluation-only | `core/tests/kernel_properties.rs`, `engine/tests/reference_loop.rs` |
| Conversation UI | `tui/src/v2app.rs`, `tui/src/v2ui.rs`; rendering and mouse hit-testing share geometry | `tui/tests/v2app_tests.rs`, `make pty` |
| Install and release | `install.sh`, `engine/src/cli.rs`, `.github/workflows/release.yml` | `engine/tests/install.rs`, CLI tests, `make release-rehearsal` |

The TUI reaches the engine through the daemon socket, not the database. Extract modules around independent
responsibilities and real change rates; do not add a framework to silence a lint or make a one-off abstraction.

## Stable regression tests

Integration tests isolate `XDG_CONFIG_HOME` and `XDG_STATE_HOME` through child processes (`Command::env`, see
`engine/tests/cli.rs`). If a test must change its own process environment, change only its variables and
restore them in the same test. Engine library tests use `crate::env_lock()` where appropriate.

Keep project, config and state directories separate beneath a temporary root. Do not use a directory containing
XDG state, or `/tmp` itself, as a production-session workspace. Tests must not depend on the machine's user
config, test-name ordering or responses that precede requests.

Use barriers or channels for concurrency, and join every thread and child before tearing down fixtures. Fake
services should read each request before answering.

## Code review and evidence

- Keep large formatting sweeps separate from behavior changes. Never disable a lint crate-wide; use a scoped
  `#[expect(..., reason = "...")]` only when the interface genuinely requires it.
- File opens must state whether they keep, append or truncate. Do not turn lock files into truncating opens to
  silence a lint.
- Test new logic for success, failure and recovery. Pure moves and formatting reuse existing regressions.
- `review/tmp/` is ignored scratch space. Durable conclusions belong in dated reports; frozen evaluation
  evidence belongs under `review/eval/r2-p6/runs/`.
- Do not commit Python caches, temporary databases, nested repositories, build output or copies of old source.
  Temporary files created by a review belong in `/tmp` or `review/tmp/`.
- Read-only reviews must not modify reviewed files. Every falsification claim needs a reproducible command or
  probe and must first rule out probe error.

## Documentation maintenance

Give each document one job:

| Document | Owns | Do not use it for |
|---|---|---|
| README and Chinese mirror | Current introduction, installation and important boundaries | Complete reference or benchmark proof |
| USER-GUIDE and INSTALL | Commands and behavior available now | Future features or retired behavior presented as current |
| DESIGN and DECISIONS | Requirements, contracts and dated decisions | Letting an older entry override a newer one |
| ACCEPTANCE and CAPABILITIES | Evidence and its limits | Treating local tests as live-service acceptance |
| PRODUCT-COMPARISON | Dated comparison and one candidate queue | Implementation, spend or release authorization |
| HANDOFF | Latest baseline, focused checks and preservation rules | Remote/install identity or blanket work authorization |
| Dated reports and manifests | Evidence for their recorded sources and runs | Claims that later source passed the same run |

When behavior changes, update its user-facing home, evidence boundary and handoff together. Link to shared
lists instead of copying them. Record the inspected commit and actual run date. Distinguish implemented,
partially verified, absent and intentionally excluded capabilities. Correct current summaries without
rewriting frozen receipts or regrading historical runs.

Write for the reader: state current behavior first, use direct sentences, and keep decision chronology in
DECISIONS or dated reports. Do not repeat the same caveat in several sections or narrate how a document was
assembled. Use one H1, descriptive headings, relative links and consistent terms. Keep the two READMEs aligned
on headings, links, CLI flags, Ctrl chords and slash commands; `review/readme_zh.py --self-check` checks the
comparison itself.

Regenerate references after changing their source:

```bash
python3 review/config_reference.py --write
python3 review/event_catalogue.py --write
python3 review/protocol_catalogue.py --write
python3 review/tool_catalogue.py --write
python3 review/test_counts.py --write   # after suite-size changes; updates all governed statements
```

Run `make hygiene` and `git diff --check` for documentation edits, and `make check` before committing. Extend
an audit when it misses a real interface; do not weaken its validation to accept drift.

## Dependencies, toolchain and releases

Each crate commits its own `Cargo.lock`, and checks and release builds use `--locked`. Prefer the standard
library, then existing dependencies, before adding a new one. Update only affected lockfiles and rerun the
relevant interface checks and `make check`.

A Rust upgrade means changing `rust-toolchain.toml` and rerunning `make fmt`, `make check` and `make pty`.
CI and release builds read the same pin.

**Published release:** v0.2.0 is the first public release (2026-10-22, Linux x86_64 musl-static). A future
release requires explicit authorization and the workflow repository variable TEAMAGENTS_RELEASE_APPROVED=true.
The variable alone
is not authorization.

For a future release:

1. Bump all three crates and their lockfiles. `review/release_artifact.py` checks version agreement.
2. Update `.github/release-notes.md`, pass `make check` and `make pty`, and run the required probes.
   `review/build_references.py` checks workflow file references; `review/test_counts.py` keeps suite counts
   aligned.
3. Push the authorized `vX.Y.Z` tag. The workflow checks the tag/version, builds musl binaries, assembles and
   checksums the archive, and smoke-installs that exact archive before the publication gate.
4. Rehearse without publishing with `make release-rehearsal`. This is a build and can take several minutes on
   a cold cache; `review/release_rehearsal.py` implements the local steps.
5. After publication, update INSTALL, both READMEs, ACCEPTANCE, CAPABILITIES and HANDOFF with the exact
   artifact and source. `review/install_check.py` verifies the published artifact; it does not relabel history.

The D-451 rehearsal remains available as `python3 review/maturity-2026-10-11/install.py`, with hashes recorded
by `review/maturity-2026-10-11/run.py`. It uses fresh HOME/XDG/bin/workspace roots and does not replace the
user's installation or publish a tag.

## Fixed tasks and grading

The fixed task set and runner live in `review/eval/r2-p6/`. Each task provides `prompt.md`, `checks.txt` and an
optional `fixture/`. `run.py` gives every trial fresh work and state directories, grades inside that same
work directory and writes `runs/<date>/results.jsonl`. See the [evaluation guide](../review/eval/README.md).

- Freeze task inputs and grading before a run; manifests record their hashes.
- Check disk space first. Large build output can reach several GB; use an ignored output directory and, when
  needed, point `TMPDIR` at the same filesystem.
- Never rewrite existing evidence or convert an old failure into a success by moving storage.
- When comparing products, isolate hidden tests with an external filesystem boundary and verify it with a
  model-free probe. TeamAgents no longer provides that boundary itself.
- If hidden material enters a model's context, stop and retain the polluted trace. Rerun with fresh inputs
  rather than continuing from contaminated context.

## Real-model verification

Real-model runs need explicit credentials, authorization and the model's native context. They are not part of
`make check`.

```bash
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<new date>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<new date>
engine/target/debug/teamagents exec --json --timeout 180 "1+1=?"
engine/target/debug/examples/eval_group_a --task "..." --workdir /tmp/t --trace /tmp/t-trace
make probe-offline
make probe-models
python3 review/dogfood/checks.py --state-dir /tmp/ta-checks
```

`review/dogfood/README.md` describes the probes. `make probe-offline` selects credential-free checks;
`make probe-models` runs model-requiring checks serially. `python3 review/dogfood/probes.py --list` shows both
sets. The harness keeps failed-run state for diagnosis and applies the same leak rules as the test suites.

Use fresh work/state directories and the model's native context window, recording its value and source.
DeepSeek Flash uses the user-confirmed 1M window. A run with a deliberately shrunken real-model window is not
valid acceptance evidence.

Follow the preregistered grading criteria. Small samples, intervals containing zero or high variance call for
an inconclusive result, not a claim of parity or improvement. Operational recovery notes are in
[AGENTS.md](../AGENTS.md).
