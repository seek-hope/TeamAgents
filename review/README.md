# review: reviews, evaluations and evidence

This directory holds **re-runnable checks and their raw evidence**. Conclusions need a command or a probe, and
tests are recorded separately from evidence.

| Path | Contents |
|---|---|
| [`eval/`](eval/README.md) | fixed tasks and real-model evaluation (current entry `eval/r2-p6/run.py`, raw results in `eval/r2-p6/runs/`) |
| `eval/r2-p6/runs/` | the raw JSONL, grading logs and per-trial state left by each run (the **only** source of the conclusions) |
| [`dsec-kernel-reference-2026-09-24.md`](dsec-kernel-reference-2026-09-24.md) | notes comparing an external sandbox platform with this kernel design |
| [`fix-notes-verification-2026-09-24.md`](fix-notes-verification-2026-09-24.md) | ledger of the issues formal verification found and their fixes (see `verification/REPORT.md`) |
| [`dogfood/`](dogfood/README.md) | run the built CLI on this repository's own fixtures with their acceptance commands as completion checks, and `dogfood/authority.py` (the user's authority surface: grant → a worker runs a command → revoke) `dogfood/providers.py` (a team spanning DeepSeek and Kimi, A27), `dogfood/checks.py` (a required check that can never pass, A16), `dogfood/runtime_note.py` (a turn after a settlement still rides the wire, D-71) `dogfood/queued_input.py` (a queued input's run reports its own outcome, D-72) `dogfood/mcp.py` (a configured MCP service is bound and called, D-74) `dogfood/workspace.py` (the git-worktree lifecycle, D-76) `dogfood/skills.py` (the configured Skills registry reaches the model, A26) `dogfood/web.py` (the bound web tools and their private-address guard, D-79) `dogfood/crash.py` (a daemon crash replays nothing, A08/A11) `dogfood/tui.py` (the surface a user opens first: the real TUI on a real daemon with a real model, D-85) and `dogfood/input_latency.py` (per-keystroke composer latency against the scripted daemon, D-85); real model where the probe needs one, not part of `make check` |
| `install_check.py` | verify the documented install path end to end: the published release downloads, its SHA-256 matches, `install.sh` installs both binaries and they run, and a corrupted archive is refused without touching an existing installation (needs network and a published release; not part of `make check`) |
| `tmp/` | ignored probe and scratch area (never committed) |

Reviews from earlier implementations and their migration are no longer in the tree; they stay reachable
through Git history (`git log -- review/archive`) and do not describe the current code. Current behaviour is
defined by [`docs/DESIGN.md`](../docs/DESIGN.md), [`docs/USER-GUIDE.md`](../docs/USER-GUIDE.md) and
[`docs/ACCEPTANCE.md`](../docs/ACCEPTANCE.md). A read-only review never modifies the reviewed files, and a
falsification claim must first rule out probe error.
