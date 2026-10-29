# review: checks, evaluations and evidence

This directory contains re-runnable checks, review notes and raw evidence. Dated reports describe their pinned
source and runs; they do not establish that later source passed the same checks. Current local regression and
open acceptance boundaries live in [ACCEPTANCE](../docs/ACCEPTANCE.md). Candidate work has one queue in
[PRODUCT-COMPARISON](../docs/PRODUCT-COMPARISON.md).

Every conclusion needs a reproducible command or probe. Read-only reviews do not modify the reviewed files;
before claiming a falsification, rule out probe error. Current behavior is defined by
[DESIGN](../docs/DESIGN.md), [USER-GUIDE](../docs/USER-GUIDE.md) and the source, with
[DEVELOPMENT](../docs/DEVELOPMENT.md) as the maintenance entry point.

## Reports and datasets

| Path | Contents and boundary |
|---|---|
| [Goal autonomy source study](goal-autonomy-2026-10-28/REPORT.md) | Seven pinned repositories and the original DD-11 proposal, now implemented as D-487; no comparative model trial |
| [Coding workflow maturity](maturity-2026-10-11/REPORT.md) | D-451 implementation and immutable local receipts, pinned to its recorded source; later D-452 fixes are in DECISIONS |
| [Codex/Pi comparison](parity-2026-10-08/REPORT.md) | D-450 presentation fixtures, source provenance and verification receipts; historical comparison |
| [Evaluation guide](eval/README.md) | Fixed tasks and real-model evaluation entry points; raw results under `eval/r2-p6/runs/` |
| [Benchmark ledger](benchmark/README.md) | Terminal-Bench, SWE-bench and tau3 harnesses, results and corrections; [failure analysis](benchmark/CAPABILITY.md) |
| [Dogfood probes](dogfood/README.md) | CLI/daemon/TUI probes, their credentials and assertions; not all are part of `make check` |
| [Sandbox design note](dsec-kernel-reference-2026-09-24.md) | Historical comparison with an external sandbox platform; not current TeamAgents execution |
| [Pi durable reference](pi-durable-reference-2026-10-02.md) | Notes on Pi 1.0.0 and `@earendil-works/pi-durable` 1.0.0, with ideas considered for this kernel |
| [Verification fix notes](fix-notes-verification-2026-09-24.md) | Issues found by formal verification and their fixes; see `verification/REPORT.md` for the formal report |

Raw JSONL, grading logs and per-trial state under `eval/r2-p6/runs/` are evidence of those runs. Do not rewrite
them to match current code or a preferred conclusion. `tmp/` is ignored scratch space and is never committed.

## Audit entry points

`make hygiene` runs the source/document audits. [DEVELOPMENT](../docs/DEVELOPMENT.md) lists every gated script
and explains the maintenance workflow. The groups below are an index, not another inventory to keep in sync.

| Area | Scripts |
|---|---|
| Build and document inventory | `build_references.py`, `hygiene_catalogue.py`, `doc_inventory.py`, `citations.py`, `markdown_tables.py` |
| Decisions and requirements | `decisions_log.py`, `decision_queue.py`, `decision_citations.py`, `requirement_trace.py` |
| User-facing contracts | `project_config_claim.py`, `release_artifact.py`, `readme_zh.py`, `doc_flags.py`, `exec_report.py`, `tui_keys.py`, `env_knobs.py` |
| Config and generated references | `config_keys.py`, `config_reference.py`, `command_params.py`, `flag_fields.py`, `event_catalogue.py`, `protocol_catalogue.py`, `tool_catalogue.py` |
| Source and test quality | `dead_code.py`, `silent_skips.py`, `leak_guard.py`, `test_counts.py` |
| Evaluation and formal material | `eval_manifests.py`, `eval_surface.py`, `eval/r2-p6/anatomy.py`, `verification_catalogue.py` |
| Probe selection | `dogfood/probes.py` |

There are now **18** probes in the offline set, selected by `dogfood/probes.py`. For the current tree, `citations.py` verifies that 160 relative links resolve today. It checks 722 commands, 26 distinct targets, 20 declared as checked on 2026-10-29;
its other totals are recomputed and compared with the script's own docstring.

Useful manual checks:

```bash
python3 review/readme_zh.py --self-check
python3 review/doc_inventory.py --self-check
python3 review/leak_guard.py --self-check
python3 review/dogfood/probes.py --self-check
python3 review/dogfood/probes.py --list
```

`release_artifact.py` distinguishes three publication states: no release, an older product layout and the
current v2 layout. `install_check.py` downloads and installs the published release, verifies its checksum and
checks that a corrupt archive leaves an existing installation untouched. It needs network access and is not a
local acceptance gate.

`host_cleanup.py` is a read-only host process census. `runner_cost.py` measures runner overhead.
`codex_surface.py` and `comparison_sources.py` support dated comparison work and require their respective
local binary or network sources. None of these manual tools belongs in an offline `make check` run.

## Evidence rules

- Keep immutable receipts and their source pins intact. Add a new dated run for new source instead of editing
  an old manifest or upgrading a historical verdict.
- Keep credentials out of reports, fixtures, events and logs. Test-only fake services must not need real keys.
- Preserve failed-run state when it is needed for diagnosis. Do not use broad process-pattern kills or delete
  user sessions to make a probe clean.
- Local tests, scripted providers and PTY checks answer different questions from real-provider, OAuth,
  graphical-terminal or cross-product acceptance. State which one was run.
- Historical review batches removed from the tree remain available through Git history
  (`git log -- review/archive`); they do not describe current code.

<!-- The reading map (D-443): the review material is evidence, one home each. The three dated notes are kept
     because decisions and the verification report cite them, and ACCEPTANCE Q19 keeps review evidence. -->
<!-- maps: review/goal-autonomy-2026-10-28/REPORT.md reader=evidence -->
<!-- maps: review/parity-2026-10-08/REPORT.md reader=evidence -->
<!-- maps: review/maturity-2026-10-11/REPORT.md reader=evidence -->
<!-- maps: review/benchmark/README.md reader=evidence -->
<!-- maps: review/benchmark/CAPABILITY.md reader=evidence -->
<!-- maps: review/dogfood/README.md reader=evidence -->
<!-- maps: review/dsec-kernel-reference-2026-09-24.md reader=evidence -->
<!-- maps: review/fix-notes-verification-2026-09-24.md reader=evidence -->
<!-- maps: review/pi-durable-reference-2026-10-02.md reader=evidence -->
