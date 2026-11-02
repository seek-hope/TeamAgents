# D-494 robustness, performance and formal coverage review

Date: 2026-11-03. Source baseline: `5f66a70d`; implementation and delivery pins are recorded in
`docs/HANDOFF.md`. The user authorized repository-wide performance, security and stability optimization
and explicitly requested a formal-coverage check. This is a targeted implementation review with executable
regressions, not an independent security audit or a real-model coding benchmark. HarnessMM is excluded.

The review covered provider transport/parsing, job execution/recovery, storage and supervision, TUI rendering
and socket behavior, plus the complete formal catalogue and verdict gates. It preserves the control-plane
transaction, authority, budget, unknown-outcome and native-context contracts. Ten documentation files were
already modified at the start; those changes are preserved separately from this implementation.

## Confirmed defects and changes

| Area | Reproduction and consequence | Change and executable bridge |
|---|---|---|
| Dependency advisory | The refreshed RustSec database reported RUSTSEC-2026-0433 for pulldown-cmark 0.13.5: quadratic nested-image wikilinks parsing. The renderer does not enable wikilinks, but the dependency gate still rejects the affected version. | Upgrade the TUI manifest floor and lockfile to 0.13.6; re-run advisory, TUI and terminal gates. |
| Provider bodies | The old adapters allocated the entire non-SSE/error body before checking its size. A chunked stream can exceed the limit without EOF. | Shared collection checks declared length and every chunk against 64 MiB and honors cancellation. A pending-after-overflow stream must fail immediately; exact-limit bodies remain valid. `engine/src/providers/stream.rs` tests. |
| SSE framing | Valid CRLF/multiline events were rejected, and decoding HTTP chunks independently could corrupt split UTF-8. | Byte framing preserves partial code points, accepts LF/CRLF/CR and joins data fields. Every possible byte split of a Unicode/multiline fixture is checked. Incomplete events never dispatch; only complete data events renew the stall deadline. |
| Anthropic content | A non-object content block followed by a delta caused a serde_json indexing panic. | Reject malformed blocks before mutation. Three provider integration regressions failed on the old implementation and pass after the change; `engine/tests/cases/provider_robustness.rs`. String deltas now append in place. |
| Runner connections | An authenticated connection left open after a reply blocked all supervision and other clients. Recreating the timer on traffic could indefinitely postpone ticks. | A bounded set of socket tasks feeds one serialized command loop; its supervision timer is independent. Frames are capped at 64 KiB, connection tasks at 32, and frame/exchange waits at two seconds. `engine/tests/cases/jobs_robustness.rs`. |
| Cancellation | Repeated cancel moved the grace deadline; reaping a TERM-exiting leader abandoned a TERM-resistant descendant in the same group. Both failed old-code regressions. A late poll also relabeled an already completed, unreaped leader `CANCEL_REQUESTED`. | Preserve the first cancel timestamp and keep the leader unreaped until the 500 ms grace expires. Recheck its recorded identity and send group SIGKILL before reap. Normal successful commands still leave intended background services alive; a completed leader is reaped before initiating a new timeout at a late poll. |
| Durable start | Injecting a failed START_ACCEPTED journal write still launched the old command and created its effect file. | Refuse launch unless acceptance persisted. A failed attempt records failure honestly when possible and never claims a saved receipt after a failed write. The regression verifies both the error and absence of the effect. |
| Retirement | Cancel-before-start, spawn failure and recovered unknown could lack a terminal timestamp, preventing idle retirement. | Record the terminal timestamp on each path; a real runner must exit after its test grace while its journal remains readable. Recovery retains OUTCOME_UNKNOWN and never replays the command. |
| Output preview | A six-byte preview of a sparse 1 GiB log returned empty under a 256 MiB address-space cap because the old code first read the whole file. | Bound the file read itself. The real implementation returns the prefix under the same child-process memory limit. The old extracted function fails that assertion; its product behavior was an empty preview, not a claimed product crash. |
| Storage backpressure | With a blocked worker and a full one-slot queue, discovery exited permanently. A later create-instance command committed, but the new instance got no driver; shutdown reported backpressure. | Retry rejected discovery reads at the normal cadence. Always notify/drain owned drivers when discovery ends, including fatal errors. `engine/tests/cases/supervisor_robustness.rs`. No submitted mutation is replayed. |
| Idle polling | SQLite tracing observed 15 full-context reads during 250 ms of idle polling after an assistant reply. | Share the indexed visible-tail metadata query with diagnostics; load bodies and expand images only when a turn is needed. The same regression observes zero idle body reads and verifies that new input still loads context. |
| TUI redraw | A cache hit still cloned every wrapped line before selecting the viewport; wrapping allocated a String for each grapheme. | Borrow the cached base document and transient tail, copying only visible rows; preserve global media offsets and clipping. Borrow grapheme text until constructing output spans. Existing stream, summary, scrolling, history and media tests exercise the layout. |
| TUI socket deadline | A timeout on each individual read could be renewed by slow partial frames. | Share one absolute deadline across a round trip and require a complete newline-delimited frame. A slow-byte regression cannot extend the deadline indefinitely. This does not introduce a size cap on the unpaginated daemon event API. |

Cancellation still does not prove all effects stopped. The tests cover local recorded process groups, not
escaped descendants, remote effects or arbitrary host isolation. No unknown receipt is rewritten as success.
Ordinary Shell and local MCP retain D-480's host execution contract.

## Measured rendering cost

The fixture is reproducible with:

```bash
cargo run --offline --locked --manifest-path tui/Cargo.toml --example redraw_probe
```

It uses a debug build, an 80-by-24 TestBackend, 100 warm redraws with alternating composer text, and user
entries containing 500 short lines each. A counting allocator records allocations/reallocations requested
by this process; allocated bytes are cumulative requests, not peak resident memory. No model or daemon is
called. Before/after runs used the same fixture and machine without concurrent heavy formal work. Both used
pulldown-cmark 0.13.5, before the subsequent advisory update to 0.13.6.

| Entries / wrapped rows | Median frame before / after (ms) | p95 before / after (ms) | Allocated bytes per frame before / after |
|---|---|---|---|
| 40 / 20,159 | 7.958 / 0.848 | 9.135 / 0.937 | 4,922,248 / 23,210 |
| 200 / 100,799 | 39.552 / 1.369 | 42.627 / 1.650 | 24,535,688 / 23,210 |
| 400 / 201,599 | 77.206 / 1.812 | 83.295 / 2.197 | 49,052,488 / 23,210 |

At 400 entries, allocation/reallocation calls fall from 602,492 to 144 per frame. The 390,000-byte Unicode
wrapping fixture produces the same 5,000 rows; its time changes from 143.234 to 112.972 ms and allocation
calls from 540,057 to 205,057. Raw observations are in [before.json](before.json) and [after.json](after.json).
These are single-run synthetic debug measurements, not release latency guarantees or statistical population
estimates. Full-history content hashing remains linear; parsing after invalidation and arbitrary media-heavy
histories have not been shown constant-time.

## Formal coverage result

The final exhaustive targets verify 50 positive configurations and refute 273 expected negative controls
across 41 modules. Seven Kani harnesses pass. One additional wide configuration is inventoried but is not a
completed exhaustive proof. The formal catalogue now checks positive claims independently of negative
configurations; its missing-positive and missing-expected-claim controls run during hygiene.

The verdict gate requires both exit status and the expected result and rejects warnings on every exhaustive
target. A generic temporal violation is accepted only for a configuration whose sole temporal property is its
expected claim. Pinned TLC also reports named action-property failures with a temporal exit status and
constant-FALSE invariants as a special configuration diagnostic; the parser checks those exact cases. The
latter are static refutations, not explored traces. Early parser failures during this work are not counted as
product/model counterexamples. Existing negative configurations were not weakened.

`V2JobSupervision` supplies nine controls for durable acceptance, socket starvation, timer resets, repeated
cancel, early leader reap, missing retirement time, retirement liveness, normal service preservation and
already-observable completion winning over a late deadline tick.
`V2Discovery` supplies two controls for transient pressure and error cleanup. `V2Daemon` now records an
application count and checks `CommandApplicationCountIsBounded`; its historical `AppliedAtMostOnce` name
still denotes receipt presence and cannot alone substantiate deduplication.

The audit found no declared claim missing from a positive configuration. The ledger has zero unexplained
missing behavioral controls and four documented entailments, with type predicates separately listed.
This establishes catalogue completeness against its declarations. It does not establish that the declarations
cover every implementation path or every requirement. The current domain matrix, retired-model boundary,
wide-state-space limit and unproven list are in [the formal report](../../verification/REPORT.md).

Re-run the evidence gates with:

```bash
make check
make pty
make deny
make verify-model-all
make verify-model-counterexamples
make verify-kani
python3 review/verification_catalogue.py --self-check
python3 review/tlc_verdict.py --self-check
```

The Rust suites and terminal matrix are green: core 174 / engine 459 / tui 104 (737 tests), with the leak
guard, formatting, Clippy, hygiene and dependency-advisory checks passing. A separate staged checkout passes
hygiene without the ten pre-existing documentation edits. That check exposed an event-catalogue path bug:
it treated every file below an absolute `/tmp` checkout as scratch. Scratch exclusion is now relative to the
repository, preserving the intended exclusion while scanning that checkout normally.

The provider, runner and supervisor regressions also run independently through their `providers_fake`,
`jobs_runner` and `v2_supervisor` Cargo integration targets. The final source/PTY/advisory/installation results
are recorded in `docs/ACCEPTANCE.md` after completion; passing targeted tests does not substitute for them.

## Delivery evidence

Material source: `0027ffe2ffbbba57acf852f59bf8904dce5b0ac3`. The complete 50-positive/273-negative TLC
targets passed again on this committed source. Native release builds passed and replaced the installed
binaries after a private rollback copy. No installed/user-root daemon was running at preflight. No session
database was present under the user state root to snapshot; this installation did not create, migrate or
delete a user session, and left the user config unchanged.

| Installed binary | SHA-256 |
|---|---|
| teamagents | `37d9743f62bc0f6019a1d7e71105b51c0362d85e8597d913d1a36a25c8011ec7` |
| teamagents-tui | `216c8ded6124af29f0f844d4266d0626676e1d5646453ae30128c572511638e3` |

Installed smoke checks use temporary config/state/workspaces and a loopback model endpoint. Init remains
idempotent, the fresh schema is 7, the idle daemon creates zero model requests and zero HTTP calls, and its
normal shutdown succeeds. The actual installed TUI passes the conversation fixture and forced blocks/Kitty
graphics PTYs. These fixtures never start a user task or a user-root daemon. Version remains 0.2.0; the
public D-493 release and tag were not changed.

## Remaining boundaries

- No full-system composition, Rust/SQLite refinement, universal liveness, filesystem power-loss proof or
  arbitrary OS/process-scheduler proof was added. Fairness and bounded domains remain model assumptions.
- Provider wire parsing, OAuth/HTTP/SSRF behavior, rendering dependencies and operating-system interactions
  have executable samples and source review, not complete formal proofs.
- The daemon event API remains unpaginated. A complete shared size/deadline policy for daemon/exec replies
  needs its own protocol work; adding an arbitrary reply cap here would strand legitimate historical cursors.
- Reviewer usage is still not fully propagated into goal accounting. This known gap is not solved by transport
  hardening or a green accounting abstraction.
- No paid model evaluation, coding success-rate comparison, search-quality study, independent audit or new
  public release was performed. The published v0.2.0 remains the D-493 artifact until separately authorized.
