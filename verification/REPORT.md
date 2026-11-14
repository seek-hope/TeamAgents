# Current formal verification report

This report records the currently accepted finite verification evidence and its limits. It is not a history of
earlier runs. The property/configuration mapping is in [README.md](README.md); executable reproduction commands
are in [DEVELOPMENT.md](../docs/DEVELOPMENT.md).

## 0. Gate status (re-run 2026-11-14 at `3274d1fa`)

The current targets verified all **65** configurations in **62** traversals, refuted all **327** negative controls,
reached all **79** normal-path witnesses, and passed all **9** Kani harnesses. The latest complete local replay
used the current source and exited 0 for every target. Kani reported `Complete - 9 successfully verified harnesses, 0 failures, 9 total`.

`verification/tla/V2SearchTree.tla` has SHA-256
`b2d814d597ddb5f9382ca712e6431a1bf3d67ab7b21ce2f44137c5cae39a2c12`.
The complete `make check` gate also passes with **777 Rust tests**: core 191, engine 481 and TUI 105.

The catalogue contains **393 configurations across 48 modules**: 65 positive configurations, 327 controls and
one separate wide configuration. Positive inputs share a traversal only when their model, specification and
domains match; the 65/62 distinction is intentional.

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

The original 24-step control-plane exploration is retained before the two explicit tree steps, so tree coverage
does not displace existing approval coverage. These tests use real SQLite and scripted providers, not a claim of
arbitrary-input TLA enumeration.

## 3. What remains unproven or unmeasured

1. **Full refinement and composition.** No complete Rust/SQLite trace refinement, whole-system composition,
   arbitrary actor/command/bootstrap/schema/fork/retention/export composition or arbitrary-string capability
   algebra is established.
2. **Wide and canonical allocation.** The separate wide configuration is not claimed complete. Policy B
   canonical allocation is not adopted; a source-bound bidirectional fair-trace equivalence, including actual
   actions, enabledness, occurrence, fairness, properties/witnesses and exact source binding, is not complete.
3. **Scheduler and environment fairness.** Provider scheduling, process-group behavior, escaped descendants,
   remote effects, filesystem races and power-loss timing remain outside the finite model.
4. **Implementation detail outside the model.** Complete provider/JSON-RPC parsing and retry classification,
   arbitrary JSON/text domains, SQL execution, every schema migration, MCP framing/correlation and all UI/render
   paths are not formally proved by the bounded modules.
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
