# Automatic-review adversarial code review (D-502)

Date: 2026-11-10. Pre-fix source: `f9c870345da7346535f8589104a772f403bea7cd`.
Post-fix source: the commit introducing D-502 and this report. This is a focused implementation review, not a
renewal of D-452, a live-model evaluation or a general proof that automatic approval is safe.

## Method

A fresh read-only reviewer session inspected the reviewer adapter, driver and control-plane paths. Its findings
were leads, not accepted evidence. Each finding below was then reproduced against the pre-fix source with a
focused Rust test. A second read-only pass reviewed the changes and found the prompt/schema mismatch and the
large-instruction truncation composition, which received their own regression coverage.

The original context probe initially failed because its synthetic peer lacked a message grant. That probe error
was corrected before claiming a context falsification. The corrected pre-fix request contained old-epoch text
and omitted the configured policy. No production files were changed by the read-only passes.

## Reproduced findings and fixes

| Finding | Pre-fix evidence | Current contract and regression |
|---|---|---|
| Surrounding prose, missing/wrongly typed fields and contradictory `allow` responses were accepted | `d502-reviewer-unit-before` exited 101 | Whole-document strict JSON, required typed/enumerated fields, non-empty rationale; allow requires low risk and high user authorization. `engine::v2::reviewer::tests::parses_only_complete_consistent_strict_json_assessments` |
| A contradictory model allow could approve and execute | `d502-driver-review-before` exited 101 | Parse failure leaves the approval pending and the marker absent. `v2_driver::auto_review_contradictory_allow_fails_closed` |
| Old-epoch and peer-originated context could displace or impersonate user authorization; configured instruction files were absent | `d502-review-context-before` exited 101; the captured request contained `OLD-EPOCH-AUTHORIZATION` and no `CURRENT-POLICY` | Only current-epoch envelopes with kind `user_input` and sender `user` supply user facts. Configured instruction files are included, and unreadable policy fails closed. `v2_driver::auto_review_context_uses_current_user_input_and_configured_policy` |
| Head truncation discarded the newest denial; a large instruction file could then erase all user facts | `d502-reviewer-unit-before` exited 101; second-pass source inspection found the composed case | User facts and instructions have independent bounded shares, retain their newest tails and carry a truncation marker. `engine::v2::reviewer::tests::context_bound_preserves_latest_user_facts_and_policy` |
| A system `review_decision(allow)` did not require a completed matching review and could approve self-update | `d502-core-review-before` exited 101 for both cases | Allow names the operation and completed review request, repeats the risk/authorization guard and rejects `request_self_update`; denial remains available for deterministic hard denial without a model call. `automatic_review_decides_only_one_operation_approval` and `automatic_review_never_allows_self_update` |
| A fully received malformed response lost known provider usage | `d502-driver-review-before` recorded unknown instead of known usage | The adapter returns usage with parse failure; a FAILED review attempt settles known usage and releases its reservation. `v2_driver::auto_review_parse_failure_preserves_known_usage` asserts ten known tokens and zero unknown reviews |
| Simple whitespace/global-option forms bypassed deterministic hard-deny examples | `d502-reviewer-unit-before` exited 101 | Conservative lexical checks cover the reproduced publication, privilege, root-deletion and environment-credential forms. `hard_denies_publication_and_credentials_without_a_model` |

Run the current regressions with:

```bash
cargo test --offline --manifest-path core/Cargo.toml automatic_review
cargo test --offline --manifest-path engine/Cargo.toml --lib v2::reviewer::tests
cargo test --offline --manifest-path engine/Cargo.toml --test v2_driver auto_review_
```

Focused after-fix receipts `d502-core-review-after`, `d502-reviewer-unit-after`, `d502-driver-review-final` and
`d502-reviewer-boundary-after` exited 0. Full core and engine suites also exited 0 before the final context-bound
refinement; the final repository gate is recorded in ACCEPTANCE rather than inferred from those earlier runs.

## Boundaries

- The lexical hard-deny list is a conservative backstop, not a shell parser or complete safety policy. It must
  not be cited as proof that every dangerous command spelling is recognized.
- Scripted adversarial responses establish parser, authority, context and accounting behavior. They do not
  establish how often a real reviewer model makes a sound judgement or resists a novel prompt injection.
- No real provider, external OAuth service, paid comparison or price table was used. Token usage remains
  provider-reported usage, not a dollar estimate (D-459).
- The new control-plane guards are tested here but are not thereby added to the existing TLA proof boundary.
