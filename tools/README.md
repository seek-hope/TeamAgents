# Current development and verification tools

This directory contains executable development, verification and diagnostic tools used by the current source.
It contains no review reports, evaluation datasets or archived evidence.

## Entry points

| Area | Contents |
|---|---|
| Build and document checks | `build_references.py`, `hygiene_catalogue.py`, `doc_inventory.py`, `citations.py`, `markdown_tables.py` |
| Decisions and requirements | `decisions_log.py`, `decision_queue.py`, `decision_citations.py`, `requirement_trace.py` |
| User-facing contracts | `project_config_claim.py`, `release_artifact.py`, `readme_zh.py`, `doc_flags.py`, `exec_report.py`, `tui_keys.py`, `env_knobs.py` |
| Generated references | `config_keys.py`, `config_reference.py`, `command_params.py`, `flag_fields.py`, `event_catalogue.py`, `protocol_catalogue.py`, `tool_catalogue.py` |
| Source and test quality | `dead_code.py`, `silent_skips.py`, `leak_guard.py`, `test_counts.py` |
| Formal verification | `verification_batches.py`, `verification_catalogue.py`, `verification_requirements.py`, `verification_witnesses.py`, `tlc_verdict.py` |
| Installation and diagnostics | `release_rehearsal.py`, `release_artifact.py`, `install_check.py`, `host_cleanup.py`, `runner_cost.py` |
| Product probes | [dogfood/README.md](dogfood/README.md) scripts and `dogfood/probes.py` selection/self-checks; now **18** probes in the offline set |

`make hygiene` runs the source/document audits. `make check` runs the complete offline gate. `make verify-model-all`,
`make verify-model-counterexamples`, `make verify-witnesses` and `make verify-kani` run the formal checks.
`make pty` runs the terminal matrix. See `docs/DEVELOPMENT.md` for the full commands and their boundaries.

For the current tree, `citations.py` verifies that 86 relative links resolve today. It checks 88 commands, 14 distinct targets, 14 declared as checked on 2026-11-14; its other totals are recomputed and compared with the script's own docstring.

`tools/tmp/` is ignored scratch space for new local runs. It is not source, evidence of a later source revision or a
committed artifact. A passing result always needs its actual command, source revision and exit status.

<!-- maps: tools/dogfood/README.md reader=developer -->
