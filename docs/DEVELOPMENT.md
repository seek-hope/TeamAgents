# Development

This is the current development entry point. The implementation is Rust (`core`, `engine`, `tui`); Python is used
only for test tooling and fake services. Code and documentation are English, except `README.zh-CN.md`.

## 1. Main commands

```bash
make check CARGO_FLAGS=--locked   # first run may download dependencies
make check                       # offline after dependencies are available
make fmt                         # format all crates and included test files
make deny                        # dependency advisories and source policy
make pty                         # isolated real-terminal matrix
```

`make check` is the required pre-commit gate: formatting, Clippy with `-D warnings`, all three test suites and
repository hygiene. Tests isolate their state roots. Do not weaken a failed assertion or replace it with a
simulation or plan.

Useful focused commands:

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml
python3 tools/test_counts.py
python3 tools/citations.py
make language-check
```

Current ledger: **core 191 / engine 481 / TUI 105 = 777 tests**. `tools/test_counts.py` checks that the source
suites and documented counts agree.

## 2. Formal verification

```bash
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
```

The first formal run may download the pinned **TLC v1.7.1** jar; its SHA-256 is pinned in `Makefile`.
`TLA_METADIR` defaults to `tools/tmp/tla`; use a separate
scratch directory for manual probes. Do not point TLC at `verification/tla/states` or commit generated states.

Current material: **393 configurations across 48 modules**, comprising **65 positive configurations in 62
traversals**, **327 negative controls** and one separate wide configuration. There are **79 normal-path witnesses**
and **9 Kani harnesses**. Positive configurations share a traversal only when model/spec/domain match.

A witness demonstrates reachability, not safety. A positive TLC root must be semantically processed with no hidden
warning, omitted proof or abort. A skipped, timed-out or unsupported proof is not rewritten as a pass. The wide
configuration is not claimed complete, and Policy B canonical allocation is not adopted.

`verification/tla/V2SearchTree.tla` covers single and ordered two-operation patches. Its 23 controls cover every
non-`TypeOK` tree claim; 11 witnesses cover normal paths. Arbitrary strings, SQL details and live-model semantics
remain implementation-test obligations, not arbitrary-input TLA proofs.

## 3. Real terminal and product probes

```bash
make pty
make probe-offline
```

PTY runs one isolated pty per profile: xterm-256color, screen-256color, tmux-256color, xterm-kitty and Kitty
inside tmux, plus forced blocks and Kitty protocol modes. It checks protocol bytes, input ownership, resize,
graphics bounds and cleanup, not a complete graphical-terminal or OS matrix.

`tools/dogfood/` contains product probes and `tools/dogfood/probes.py` selection/self-checks. `make probe-offline`
uses no real credentials. Paid model probes are not part of `make check` and need separately bounded credentials
and scope. Real-model evaluation must use the model's native context window.

## 4. Documents and generated references

Run the relevant generator after changing source contracts:

```bash
python3 tools/protocol_catalogue.py --write
python3 tools/tool_catalogue.py --write
python3 tools/event_catalogue.py --write
python3 tools/config_reference.py --write
python3 tools/verification_catalogue.py --write-coverage
python3 tools/citations.py
python3 tools/markdown_tables.py
python3 tools/test_counts.py
```

| Area | Current audit entry points |
|---|---|
| Build and document inventory | `tools/build_references.py`, `tools/hygiene_catalogue.py`, `tools/doc_inventory.py` |
| Decisions and citations | `tools/decisions_log.py`, `tools/decision_queue.py`, `tools/decision_citations.py`, `tools/citations.py`, `tools/requirement_trace.py` |
| User and interface documents | `tools/project_config_claim.py`, `tools/release_artifact.py`, `tools/readme_zh.py`, `tools/doc_flags.py`, `tools/exec_report.py`, `tools/tui_keys.py`, `tools/env_knobs.py`, `tools/markdown_tables.py` |
| Configuration and control surface | `tools/config_keys.py`, `tools/flag_fields.py`, `tools/command_params.py`, `tools/config_reference.py` |
| Generated tool/protocol references | `tools/event_catalogue.py`, `tools/protocol_catalogue.py`, `tools/tool_catalogue.py` |
| Source and test quality | `tools/dead_code.py`, `tools/silent_skips.py`, `tools/leak_guard.py`, `tools/test_counts.py` |
| Formal material | `tools/verification_catalogue.py` |
| Probe registry | `tools/dogfood/probes.py` |

- [CONFIG.md](CONFIG.md): configuration keys and their readers.
- [PROTOCOL.md](PROTOCOL.md): daemon reads, control commands and diagnostic fields.
- [TOOLS.md](TOOLS.md): model tool schemas and availability.
- [EVENTS.md](EVENTS.md): event kinds, payloads, emitters and readers.
- [COVERAGE.md](../verification/COVERAGE.md): claims without an individual refuting control or documented exception.
- `docs/DECISIONS.md`: only current rules and the latest confirmed scope.
- `verification/REPORT.md`: current results, counts, source pin and unproven boundaries.

Do not edit generated regions by hand. `make hygiene` compares generated text with code and checks relative
links, command references, decisions, requirements, language and source inventories.

## 5. Rust and test conventions

- Prefer the standard library, then existing dependencies, then new ones. Add abstractions only for current need.
- Every non-trivial behavior needs a runnable acceptance test or self-check. Deleting unused code is preferred.
- `Control::submit` owns team transactions; model/tool waits stay outside them. SQLite uses WAL and
  `synchronous=FULL`.
- Integration tests isolate `XDG_CONFIG_HOME` and `XDG_STATE_HOME` in child processes. Tests that change process
  environment touch only their own variables and restore them in the same test.
- Cancellation is not proof of stopped side effects. Preserve `OUTCOME_UNKNOWN` and never replay unknown effects.
- Credentials come from environment variables or machine stores, never from repository fixtures, prompts, events
  or logs.
- Add a `ponytail:` comment for a simplification with a known ceiling and state its upgrade path.
- `tools/tmp/` is ignored scratch for new local probes. Do not commit scratch databases, Python caches, build
  artifacts or generated TLC states.

## 6. Environment knobs

Product behavior belongs in `config.toml`; these variables locate binaries, select diagnostics or support tests.

| Variable | Setter | Purpose |
|---|---|---|
| `TEAMAGENTS_LOG_SURFACE=1` | Operator | Log each prepared request's offered tools to the daemon log |
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

## 7. Installation and release checks

```bash
make release-rehearsal
python3 tools/install_check.py
```

`release-rehearsal` builds the current archive, verifies its checksum, installs it into an isolated location and
runs init/smoke. `install_check.py` checks the published artifact and corrupt-archive preservation. These do not
replace a genuinely clean-host, two-version install/upgrade/rollback or public-release authorization.

Do not replace installed binaries, user daemons or sessions as part of ordinary development verification.

## 7. Current open boundaries

- Live-model tree quality, reviewer judgement and general long-horizon success remain unmeasured.
- Real-provider/OAuth/public-host/graphical-terminal/clean-host and matched-Pi comparisons remain open.
- Wide exhaustive search, full Rust/SQLite refinement, whole-system composition and Policy B source-bound
  equivalence remain open.
- Scheduler/provider fairness, filesystem races, escaped descendants and remote effects remain environmental.

<!-- maps: docs/CONFIG.md reader=developer -->
<!-- maps: docs/PROTOCOL.md reader=developer -->
<!-- maps: docs/TOOLS.md reader=developer -->
<!-- maps: docs/EVENTS.md reader=developer -->
