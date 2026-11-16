# Current development and verification tools

This directory contains executable development, verification and diagnostic tools used by the current source.
It contains no review reports, evaluation datasets or archived evidence.

## Entry points

| Area | Contents |
|---|---|
| Build and document checks | `build_references.py`, `hygiene_catalogue.py`, `doc_inventory.py`, `citations.py`, `markdown_tables.py` |
| Decisions and requirements | `decision_citations.py`, `requirement_trace.py` |
| User-facing contracts | `project_config_claim.py`, `release_artifact.py`, `readme_zh.py`, `doc_flags.py`, `exec_report.py`, `tui_keys.py`, `env_knobs.py` |
| Generated references | `config_keys.py`, `config_reference.py`, `command_params.py`, `flag_fields.py`, `event_catalogue.py`, `protocol_catalogue.py`, `tool_catalogue.py` |
| Source and test quality | `dead_code.py`, `silent_skips.py`, `leak_guard.py`, `test_counts.py` |
| Formal verification | `verification_batches.py`, `verification_catalogue.py`, `verification_requirements.py`, `verification_witnesses.py`, `tlc_verdict.py` |
| Installation and diagnostics | `release_rehearsal.py`, `release_artifact.py`, `install_check.py`, `host_cleanup.py`, `runner_cost.py` |
| Product probes | [dogfood/README.md](dogfood/README.md) scripts and `dogfood/probes.py` selection/self-checks; now **18** probes in the offline set |

`make hygiene` runs the source/document audits. `make check` runs the complete offline gate. `make verify-model-all`,
`make verify-model-counterexamples`, `make verify-witnesses` and `make verify-kani` run the formal checks.
`make pty` runs the terminal matrix. See `docs/DEVELOPMENT.md` for the full commands and their boundaries.

`test_counts.py` holds the canonical ledger and registered current totals/triples to Cargo test lists. Its
`--write` mode changes numeric fields only; missing or ambiguous current statements fail. Passing-gate claims
require separate evidence bound to the actual command, source revision and exit status.

`citations.py` checks source references, relative links, Make targets and section references against the current
tree, without historical-marker exemptions. It computes coverage counts at execution time.
`verification_catalogue.py` checks the formal report against a fingerprint of the current input paths and bytes;
its source-binding controls reject changed, added, removed or renamed material.

`tools/tmp/` is ignored scratch space for new local runs. It is not source, evidence of a later source revision or a
committed artifact. A passing result always needs its actual command, source revision and exit status.

<!-- maps: tools/dogfood/README.md reader=developer -->
