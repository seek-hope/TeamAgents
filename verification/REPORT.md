# Current formal verification report

This report records accepted finite verification evidence and its limits. Property/configuration mappings are
in [README.md](README.md); reproduction commands are in [DEVELOPMENT.md](../docs/DEVELOPMENT.md).

## 0. Gate status (re-run 2026-11-23)

Material SHA-256: `c46585df510feab4db472a96d1afc7d4c85625ccccf4e2692f68267078281d75`.

The maintained targets cover all **63** configurations in **60** positive traversals, all **323** negative controls
and **79** reachability witnesses. The verified material includes the two-key profile-switch model with an optional
Worker. SQLite persistence, endpoint selection and HTTP bodies have separate executable checks.

Kani result for the identified inputs: `Complete - 9 successfully verified harnesses, 0 failures, 9 total`.
To compare the checkout with the verified material, run `python3 tools/verification_catalogue.py --source-hash`.

Re-run on 2026-11-23 because the `Makefile` gained two gate steps (`make deny` now runs
`tools/advisory_ledger.py` and `make hygiene` runs `tools/format_version.py`): `make verify-model-all`,
`make verify-model-counterexamples`, `make verify-witnesses` and `make verify-kani` all exited 0 against the current
inputs, with 60/60 positive traversals free of errors and warnings, 323/323 controls refuted unambiguously,
79 witnesses reached and all 9 Kani harnesses verified (`Complete - 9 successfully verified harnesses, 0 failures,
9 total`). The change was to the gate wiring, not to a model, a configuration or a harness.

Earlier, on 2026-11-22, `MC_inbox_stale_applied.cfg` was reduced to the one claim it refutes (it used to list
`AtMostOncePerEnvelope` too, and TLC reported whichever violation its search reached first); that re-run reached the
same 323/323, 79 and 60/60 counts.

The catalogue contains **387 configurations across 47 modules**, including one separate incomplete wide
configuration. Positive inputs share a traversal only when their model, specification and domains match;
the 63/60 distinction is intentional. The daemon model's arbitrary-prefix page abstraction does not cover
production retention, oversized-event resync, TUI refresh retry or request/input cost attribution.

## 1. Current search-tree model

`V2SearchTree` covers `read_search_tree`, `apply_search_tree` and the tree half of compaction. Its state is one
session tree, not one tree per instance. The model distinguishes User/Leader from members/system and keeps the
conversation position separate from the tree revision.

| Area | Current checked behavior |
|---|---|
| Read authority | Only User/Leader can read; a read does not modify the tree |
| Write authority | Only User/Leader can apply an ordinary patch; rejected writes preserve tree and revision |
| Revision | A fresh patch requires the current base revision; a matching cause replays without growth |
| Ordered patches | The second operation sees the first operation's result; any invalid successor rejects the whole pair |
| Structure | Root, existing parents, acyclic reachability, existing focus and the node bound hold after accepted writes |
| Merge | Target references are the union, children reparent, and a focused source transfers focus correctly |
| Freshness | Accepted writes bind the complete `(instance, epoch, entry)` position; reads report the computed stale value |
| Compaction | The summary commits on both patch outcomes; a failed patch preserves tree and revision and reports error |

Update/move/merge/focus configurations start with a valid populated tree so those operations are non-vacuous.
The add configuration covers construction from the root. The pair configuration covers second-operation
visibility, atomic rejection and replay. The 23 search-tree controls cover every non-`TypeOK` claim, including
pair rejection and pair replay. Eleven search-tree witnesses cover add/read, replay, update, move, focus, merge,
valid/invalid compaction and ordered pairs.

## 2. Executable implementation evidence

The following are implementation checks rather than TLA enumeration:

- Multi-operation generated-ID ordering, whole-patch rollback and stable replay including reconstructed
  `added_ids`.
- Arbitrary Unicode/control text: normalized JSON content is checked separately from human rendering, which
  escapes embedded breaks, terminal controls, Unicode line separators and bidi/isolate formatting controls.
- Compact JSON envelopes: nested patches, whitespace, Unicode, trailing data, wrong field types, missing or
  unknown fields and legacy text fallback.
- Real SQLite revision/head/event failures: a failed tree patch leaves no partial revision or head. A failed
  compaction tree patch does not block the summary.
- Schema migration: existing work/history is preserved, the Leader role is derived, and an unexpected
  pre-existing tree table is refused without changing the original store.
- Random executable control-plane walks: tree reads/writes are followed by root/parent/focus/reachability and
  revision-history checks after each command, including a deliberately corrupted-tree negative control.
- CLI, daemon and export projections of the same session tree.

The control-plane exploration includes 24 business transitions and two explicit tree steps, retaining both
approval and tree coverage. These tests use real SQLite and scripted providers, not a claim of
arbitrary-input TLA enumeration.

## 3. What remains unproven or unmeasured

1. **Full refinement and composition.** No complete Rust/SQLite trace refinement, whole-system composition,
   arbitrary actor/command/bootstrap/schema/fork/retention/export composition or arbitrary-string capability
   algebra is established. In particular, `bind_legacy_leader` and the schema 9→10 migration have executable
   regression tests, not a formal refinement proof. Manual `request_compaction` and `take_compaction`,
   after-work admission, temporary Worker release and schema 10→11 are likewise not formally proved.
   `rewind_conversation`, `send_rewind_draft`, `set_mcp_service` and the 11→12 and 12→13 migrations
   have no source-bound formal proof. PTY and scripted/stdio tests do not establish such a proof.
2. **Wide and canonical allocation.** The separate wide configuration is not claimed complete. Policy B
   canonical allocation is not adopted; a source-bound bidirectional fair-trace equivalence, including actual
   actions, enabledness, occurrence, fairness, properties/witnesses and exact source binding, is not complete.
3. **Scheduler and environment fairness.** Provider scheduling, process-group behavior, escaped descendants,
   remote effects, filesystem races and power-loss timing remain outside the finite model.
4. **Implementation detail outside the model.** Complete provider/JSON-RPC parsing and retry classification,
   arbitrary JSON/text domains, SQL execution, every schema migration and all UI/render paths are not formally
   proved by the bounded modules. MCP framing/correlation has executable local regression coverage but not a
   formal proof or external interoperability matrix.
5. **Tree semantics and model quality.** The model checks control transitions, not whether a model produced a
   useful or complete exploration tree, whether it reduced user lostness, or whether a summary is correct.
6. **Live acceptance.** Real provider/native-protocol, OAuth, public-host, graphical-terminal, clean-host
   install/update/rollback and matched-Pi outcome/cost comparisons are not established by this report.
7. **Independent judgement.** Reviewer judgement and prompt-injection resistance, semantic completeness of
   generated trees, and general long-horizon success rates remain empirical questions.
8. **Type-level exception.** `TypeOK` is tracked separately in [COVERAGE.md](COVERAGE.md); it is not a
   behavioral completeness claim or a substitute for a refuting transition control.

## 4. Reproduction

```bash
make check
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
make pty
```

A result is accepted only when the actual target exits 0 and the requested root is semantically processed
without a hidden warning, omitted proof or abort. A witness is reachability evidence, not a safety proof. A green
local Cargo run does not establish real-service acceptance.
