# review: reviews, evaluations and evidence

This directory holds **re-runnable checks and their raw evidence**. Conclusions need a command or a probe, and
tests are recorded separately from evidence.

| Path | Contents |
|---|---|
| [`eval/`](eval/README.md) | fixed tasks and real-model evaluation (current entry `eval/r2-p6/run.py`, raw results in `eval/r2-p6/runs/`) |
| `eval/r2-p6/runs/` | the raw JSONL, grading logs and per-trial state left by each run (the **only** source of the conclusions) |
| [`dsec-kernel-reference-2026-09-24.md`](dsec-kernel-reference-2026-09-24.md) | notes comparing an external sandbox platform with this kernel design |
| [`fix-notes-verification-2026-09-24.md`](fix-notes-verification-2026-09-24.md) | ledger of the issues formal verification found and their fixes (see `verification/REPORT.md`) |
| `tmp/` | ignored probe and scratch area (never committed) |

Reviews from earlier implementations and their migration are no longer in the tree; they stay reachable
through Git history (`git log -- review/archive`) and do not describe the current code. Current behaviour is
defined by [`docs/DESIGN.md`](../docs/DESIGN.md), [`docs/USER-GUIDE.md`](../docs/USER-GUIDE.md) and
[`docs/ACCEPTANCE.md`](../docs/ACCEPTANCE.md). A read-only review never modifies the reviewed files, and a
falsification claim must first rule out probe error.
