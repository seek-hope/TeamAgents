# Formal search optimization (2026-11-03, D-496)

The accepted changes eliminate three repeated positive traversals and avoid redundant request/subset
enumeration. They preserve the original configuration files, bounds, transitions, checked properties and
fairness. Identity allocation and explicit stuttering remain unchanged in the accepted model.

Baseline: `d7e284de9f5ebd614a938bdbb762389707d4f80d` (D-495 delivery). The baseline CI run
[41584637839](https://github.com/seek-hope/TeamAgents/actions/runs/41584637839) completed successfully for
Rust, TLA+ and Kani. D-494 remains the runtime/installed-binary source; no installation or release is involved.
Current aggregate results and the immutable formal material pin belong in
[the formal report](../../verification/REPORT.md).

## Accepted transformations

The positive runner groups configurations only when the root module identity and every non-claim
configuration line agree. It checks the union of their invariant/property names in one traversal. With a
fixed specification S, checking S implies P and S implies Q separately is equivalent to checking S implies
(P and Q). No state/view/symmetry constraint is added. Source configurations remain the inputs for the
catalogue, requirements and witnesses; named fault controls are still separate executions.

| Original positive configurations | Root module | Avoided generated states | Avoided distinct-state visits |
|---|---|---:|---:|
| `MC.cfg`, `MC_control_warn.cfg` | V2Control | 2,453,273 | 494,520 |
| `MC_daemon.cfg`, `MC_daemon_stop.cfg` | V2Daemon | 1,694,761 | 135,750 |
| `MC_checks.cfg`, `MC_checks_briefing.cfg` | V2Checks | 3,616 | 1,131 |
| Total repeated traversal | Three graphs | 4,151,650 | 631,401 |

The inventory remains 58 positive configurations, now executed in 55 traversals. D-495 generated
135,872,118 states across its 58 positive runs, so the avoided repeats are 3.06% of that aggregate.
The counts above describe avoided repeated graph visits, not a reduction in any individual specification.
The runner saves original and generated configs, model snapshots, logs, commands, exits and hashes. It
rejects unsupported syntax, fault-control inputs, indented directive keywords and duplicate config basenames.
A narrow grammar is deliberate: a new TLC directive requires extending and checking the parser first.

The request rewrite uses the identity
`exists r in U: a = r and P(r)` equals `a in U and P(a)`, retaining the membership guard before a slot
lookup. `ImportResponse` now uses all nonempty subsets of free operations for a tool response and exactly
the empty subset for another response. This is the same choice set previously obtained by enumeration
and filtering. Response kinds, outcomes, request owners and operation choices remain nondeterministic.

## Executed probes and limits

TLC release 1.7.1 (TLC2 2.16), jar SHA-256
`d532ba31aafe17afba1130f92410d9257454ff7393d1eb2fe032f0c07f352da5`.
Exploratory measurements use two workers, a 3 GiB heap, ParallelGC, fingerprint polynomial 64 and final
complete-graph temporal checking. Full positive gates retain four workers and a 4 GiB heap.

| Run | Generated | Distinct | Queue | Result |
|---|---:|---:|---:|---|
| Baseline MC.cfg with action coverage | 2,453,273 | 494,520 | 0 | Passed |
| Request/subset rewrite, same config | 2,453,273 | 494,520 | 0 | Passed |
| Old/new action equivalence on their union graph | 4,906,545 | 494,520 | 0 | Passed |
| Rewrite, MC_control_decisions.cfg | 74,053,023 | 13,846,024 | 0 | Passed, including temporal checking |
| Experimental canonical IDs, MC.cfg | 1,453,656 | 296,052 | 0 | Passed only for the experimental model |
| Experimental canonical IDs, MC_control_decisions.cfg | 22,428,733 | 4,324,760 | 0 | Passed only for the experimental model |
| Experimental removal of explicit Stutter, MC.cfg | 1,958,753 | 494,520 | 0 | Passed only for the experimental model |

The first two profiled elapsed times were 42.62 and 42.32 seconds. After the formal workloads stopped,
three serial unprofiled pairs alternated baseline/current order under the same two-worker, 3 GiB settings.
Baseline times were 31.03, 31.28 and 30.78 seconds; current times were 30.09, 30.48 and 29.08 seconds.
Medians were 31.03 and 30.09 seconds, a 3.05% reduction on this fixture. All six generated exactly
2,453,273 states and found 494,520 distinct states. This small sample is not an end-to-end CI or wide-run
speedup claim. Other exploratory runs overlapped formal workloads, so their times are not controlled
comparisons. Commands, hashes and raw measurements are in [measurements.json](measurements.json).
Equal state counts alone do not establish equivalence. The separate equivalence probe explores
the union of old/new Next relations and checks both directions of each changed action and TurnStep on every
explored edge. Its deliberately restricted reply-only variant must refute StepEquivalence. The bounded
comparison supplements the algebraic rewrite argument; it is not a general theorem about the Rust system.

Coverage attributed 495,281 generated successors and zero first discoveries to explicit Stutter.
That coverage counter differs from the final graph total and is not an additional distinct-state count.
Removing the explicit disjunct
removed exactly 494,520 generated states in the completed small run. Lifecycle changes, crash/recovery,
deadline changes and goal cancellation also generate many already visited successors. Their cycles are
part of the model, and these counts do not justify deleting their transitions.

Executed runner fixtures check the claim union and require rejection of a failing invariant or temporal
property in the second source config. Self-checks separate model/spec/constants/deadlock-policy changes and
reject unsupported directives and duplicate names. A review identified an indented directive that could
be sorted as a claim and a basename collision that could overwrite source receipts; both now fail before
TLC runs. Early fixture launches hit the pinned TLC's config-resolution behavior; copying module/config
inputs together fixed the launch, and only the successful reruns count as fixture evidence.

The original and corrected parser were also exercised directly: the original
indented-keyword fixture failed its invariant (exit 12), the unsafe sorted form passed (exit 0), and the
corrected runner rejects the input before TLC. This is a runner-development defect, not a product bug.

## Experiments that are not in the formal gate

Canonical allocation selects one unused request or attempt identity. On the successive-decisions scenario
it reduced distinct states by about 68.8%. This is promising, but it restricts labeled traces. A future
acceptance argument must establish a field-directed bijection for all reachable states, match transitions
in both directions, preserve enabledness and occurrences of every WF/SF action, and preserve every checked
state/action/temporal property and witness. Request and attempt integers overlap, so an untyped global
permutation is insufficient. Slots appear monotone in the normal model, which may support a first-allocation
renaming argument; counterfactual actions also need explicit treatment. A finite permutation probe over
one request/two attempts passed, but checks only that scenario's visited transitions and is not this proof.
Ordinary TLC symmetry reduction is not enabled for these liveness runs.

The [allocation relation probe](renaming.py) strengthens the bounded evidence. It generates every original
Next step, extends partial first-allocation maps and computes the canonical state. Reduced!Next is checked
as a property, never used to filter transitions. The one-request/two-attempt configuration completed
113,448 relational states; the two-request/one-attempt configuration completed 591,720 relational states.
These counts include allocation-map history and are not original-model state counts. Both checked pristine
unused records, monotone slot use, map injectivity, initial-state correspondence, forward successors and the
enabledness/nonstuttering occurrences of Recover, ApplyQueued and TurnStep. Mutations omitting the selected
attempt reference mapping or dropping non-reply responses both specifically refuted ForwardStep (exit 13).

The general argument still needs acceptance beyond these finite checks. For the normal model, inspection
supports the following obligations: each slot is allocated once and never recycled; nil is outside both
finite identity domains; unused records are identical and no reference names an unallocated slot. A fixed
behavior-wide bijection can then map successive first allocations to the canonical choice order. Renaming
acts on typed reference fields, not every numeric value. Finite reservation sums preserve multiplicity.
For every instance, both fairness-action enabledness and nonstuttering occurrences must correspond;
Next inclusion alone is insufficient for the converse fair-behavior inclusion. Every configured property
must be invariant under this renaming. Fault steps violate some premises and remain unreduced. A guided
witness that hard-codes IDs also needs its own run. These are proof obligations, not a machine-checked
general refinement theorem.

Removing explicit Stutter would leave the implicit stutter allowed by `[Next]_vars`, but requires an explicit
`CHECK_DEADLOCK FALSE` policy in the TLC configs. The existing explicit stutter already made deadlock checking
vacuous. The experiment demonstrates lower generated counts, not a measured wall-time improvement. It is
not included in this change, which retains the existing deadlock policy and witness inputs.

The wide configuration remains unproved; this work does not claim its exhaustion. Reducing bounds,
dropping liveness or fixing business-response branches would change the verification question. Property
abstraction/composition remains separate proof work, as do implementation refinement, arbitrary-string
capability algebra and the reviewer usage accounting gap.

## Reproduction

From the repository root, use fresh output directories:

```bash
python3 review/formal-optimization-2026-11-03/probe.py --kind baseline --coverage --out /tmp/ta-formal-before
python3 review/formal-optimization-2026-11-03/probe.py --kind current --coverage --out /tmp/ta-formal-after
python3 review/formal-optimization-2026-11-03/probe.py --kind equivalence --out /tmp/ta-formal-equivalence
python3 review/formal-optimization-2026-11-03/probe.py --kind equivalence-control --out /tmp/ta-formal-equivalence-control
python3 review/formal-optimization-2026-11-03/probe.py --kind canonical --config MC_control_decisions.cfg --out /tmp/ta-formal-canonical
python3 review/formal-optimization-2026-11-03/probe.py --kind no-stutter --out /tmp/ta-formal-stutter
python3 review/formal-optimization-2026-11-03/renaming.py --out /tmp/ta-formal-relation
python3 review/formal-optimization-2026-11-03/renaming.py --config MC.cfg --out /tmp/ta-formal-relation-requests
python3 review/formal-optimization-2026-11-03/renaming.py --fault selected-reference --out /tmp/ta-formal-relation-bad-reference
python3 review/formal-optimization-2026-11-03/renaming.py --fault response --out /tmp/ta-formal-relation-bad-response
python3 review/verification_batches.py --self-check
python3 review/verification_batches.py --check-fixtures --jar ~/.local/share/teamagents-verify/tla2tools.jar
make verify-model-all
make verify-model-counterexamples
make verify-witnesses
make verify-kani
```

The probe extracts the old model from the immutable baseline commit, refuses changed scenario configs,
records source/log/jar hashes and treats timeouts as failures. `equivalence-control` is successful only if
TLC specifically refutes StepEquivalence. The other variants require the strict positive verdict. Compare
wall time only with serial runs under equivalent resource conditions; do not compare a local probe with a
CI job or a concurrent run as evidence of speedup. Initial scratch evidence is under `review/tmp/d496/`.
