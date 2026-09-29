# R2-P6 round 8 pre-registration (the ceiling experiment, frozen before any round-8 trial)

Written 2026-09-29, **before** any round-8 trial ran. Basis: rounds 5, 6 and 7 (D-259, D-263, D-264) and
D-341's decision for them. Round 7 closed the question it opened; this round tests the *mechanism*, which is the
one thing every earlier round left as an inference.

## 1. Why a round 8 exists, stated before its data

Round 5 measured the mechanism and wrote it down: the **solo arm batches several units into one response** — one
answer that writes three units' files and runs their checks — so it is faster in wall clock than a team that
must coordinate, and its own verdict says a separating criterion "has to cap what one response can carry (a
per-response output ceiling) or use units so large that a single response cannot hold several". Round 7 then
confirmed a narrower, paired reading (a team greens a twelve-unit job earlier, 3 of 3 pairs) on a rule frozen
before its data, and closed that question.

This round removes the solo arm's batching with the treatment round 5 named — a per-response output ceiling,
applied to **every arm equally**, so it is a treatment and not a handicap — and asks whether the team arm's
parallelism still wins when one response can no longer hold several units' work.

Every earlier reading stays where it is: H1 (no regression) as recorded and not in doubt; H2's *success* form as
recorded (zero paired difference, because both arms pass every task); round 7's paired reading as recorded.

## 2. Hypothesis and decision rules (pre-registered)

| # | Hypothesis | Decision rule |
|---|---|---|
| H11 | under the ceiling, the team arm reaches **every unit verified green** earlier than the solo arm on `twelve-deliverables` | in **at least 2 of 3** fresh pairs (same repeat index) D's time is **strictly less** than B's, **and** the median paired difference is in D's favour |
| H12 | (instrument, reported first) the treatment is in force in every trial | every trial's recorded surface carries `request_options.max_tokens` equal to the manifest's `model.max_tokens`, and the pilot records the solo arm's per-response output against round 7's |

The metric is round 7's, unchanged, read by the same tool: each arm's **own** evidence that a unit is green — a
worker's `task_completed … SUCCEEDED` for D, a pytest result reporting passes for B
(`review/eval/r2-p6/anatomy.py --units`, whose self-check covers both shapes). A run whose timeline cannot be
read counts **against** H11, never silently dropped. "Strictly less" means the numbers, not a tie band: a pair
inside 1 s is reported as a tie and fails that pair.

**The bar is the smallest a real batching effect should clear, and that is stated as the limit it is.** Three
pairs cannot separate a small effect from noise — two of three happens by chance half the time — so H11 is a
*low* bar on purpose: it asks whether the direction survives removing the mechanism that favoured the solo arm,
not for a distributional separation on three samples. If H11 fails, the honest conclusion is that the gain is
**not visible once batching is capped on this task and model**, and this round does not open another; if it
holds, the reading is that the team's advantage does not depend on the solo arm's batching.

## 3. Frozen parameters

`manifest-r8.json`: `twelve-deliverables` alone (prompt, fixture and checks unchanged — the digests its earlier
manifests pinned), model `leader_main` (wire `deepseek-flash`), native context 1,000,000 (D-36),
`reasoning_effort = high`, `full_auto`, `max_retries = 2`, the 900 s wall-clock limit, **3 repeats**, **a
per-response output ceiling of 2048 tokens in both arms** (`model.max_tokens`, exported by `run.py` as
`TEAMAGENTS_EVAL_MAX_TOKENS` and recorded with every trial), the frozen experiment config in a private
`XDG_CONFIG_HOME`, and the treatment exactly as rounds 4–7's: group D is the supervisor with the collaboration
surface, the directive paragraph (D-254), the user's grant to every member (D-256), and the product's contract
text (D-255/D-257/D-258/D-262) read by both arms.

**Why 2048.** One unit's fix fits inside one response; the twelve-unit job's work does not, so a single response
can no longer answer several units, which is exactly what round 5 measured as the solo arm's mechanism. The
pilot measures the treatment in force (H12) and the ceiling is adjusted **once**, in the pilot and before any
formal trial, if the pilot shows it truncating responses in *either* arm (a turn failing with a length reason
would make the round measure truncation instead of batching). The adjustment, if any, is recorded in section 5.

## 4. What this round does **not** establish

One model, one provider, one task, three pairs per batch: it says nothing about other tasks or models, nothing
about the **end-to-end gate** (where round 5's negative result stands), and nothing about tasks whose units are
too small to batch in the first place. The ceiling is a *config* this harness sets deliberately — not a product
default — so the round says nothing about whether a user should set one. It does not re-label Q16 and does not
move any earlier reading.

## 5. Pilot (written after the pilot, before the formal round)

The pilot phase (`runs/2026-09-29-r8-pilot`, one repeat per arm, ceiling **2048**) put the treatment in force —
the trial's own recorded surface carries `request_options: {"reasoning_effort": "high", "max_tokens": 2048}` —
and it did what a pilot is for: **it caught the instrument truncating one arm**, which is precisely the case
section 3's clause names.

* The **solo arm** ran the full 900 s bound without finishing (`status: timeout`, 9 steps, 56k completion
  tokens, no length-truncation errors): with several units per response impossible, the job no longer fits the
  bound that round 7's solo trials cleared in 125–194 s.
* The **team arm** died in seconds with `Error: "issue_grant: subject/action/resource_scope must not be empty"`.
  Reading it to its cause: with `max_tokens = 2048` shared by the model's reasoning and its visible tool call,
  the arguments of a spawned member's grant call arrive empty — the `issue_grant` guard refuses them. The same
  trial run by hand **without** the ceiling completes its grants and runs normally (27 steps, 1.34M tokens,
  125 s), which is what isolates the ceiling as the cause rather than the harness or the credential.
* A first batch attempt is recorded as well, for honesty rather than as data: it launched from the wrong
  directory, so every trial read no config at all (`model key "leader_main" is not in the user catalog`,
  `wall=0.0s`) — `runs/2026-09-29-r8-formal`'s `B` trials, by contrast, *are* the instrument batch: three solo
  trials, all `timeout` at 900 s, all recording `max_tokens: 2048`.

**The single adjustment this buys, recorded before the formal round**: the ceiling moves to **4096** tokens —
still far below what several units' files would need in one response (so batching stays impossible), high
enough that reasoning plus one tool call fits, which is what 2048 broke. Everything else in section 3 stands
unchanged: same task, same model, same `limits`, same repeats, same arms. `manifest-r8.json` stays frozen at
2048 as the instrument batch's own manifest (its hash is recorded in that batch's header and must keep matching);
the formal round runs `manifest-r8-4096.json`, frozen before any of its trials, and its own batch directory.

**A formal batch at 2048 ran before this adjustment, and that is a deviation from §3's wording.**
`runs/2026-09-29-r8-formal` (started 01:03, three repeats, the 2048 manifest) produced no readable trial at
all — the solo arm timed out on all three and the team arm failed on all three with the truncation above —
and only then did the ceiling move to 4096 (01:50). §3 said the adjustment would come "in the pilot and
before any formal trial"; in the event it came after a formal batch whose own result was the same
infeasibility the pilot had shown. The batch stays in the record, and the decision entry records the
deviation (D-354).

## 6. Result (written after the formal round)

Six fresh trials under `manifest-r8-4096.json` (ceiling 4096, both arms), all accepted. Reading each arm's own
evidence that every unit is green (`anatomy.py --units`, the fixture's twelve units):

| repeat | solo arm (B): all twelve green | team arm (D): all twelve green |
|---|---|---|
| 1 | 55.6 s | **43.8 s** |
| 2 | unreadable — the bound at 900 s, 0/12 written | **42.0 s** |
| 3 | unreadable — the bound at 900 s, 0/12 written | unreadable — the bound at 900 s, 0/12 written |

`analyze.py` reads the same batch as `B 1/3 ok, D 2/3 ok -> too few samples, not confirmed`.

**H11 is not confirmed.** One of the three pairs is readable and in the team arm's favour (43.8 s against
55.6 s); the other two cannot be read at all, and section 2 counts an unreadable timeline against H11. In the
pre-registration's own terms this is the outcome the low bar still failed on, and the round reports it as such
rather than reaching for a form the data does not carry.

**H12 (instrument) is confirmed**: every trial — both arms, all six — records
`request_options = {"reasoning_effort": "high", "max_tokens": 4096}` in its own surface, and the batch header
hashes the manifest it ran.

**What the round did show, and what it did not.**

* The ceiling did **not** make the task infeasible, which the 2048 pilot could not rule out: in **one trial per
  arm** all twelve units were written and greened (B 55.6 s, D 42.0 s and 43.8 s). Both arms can finish the job
  under a cap; the cap is not a handicap aimed at one of them.
* The solo arm's batching, as round 5 measured it, is not what the cap removed here: its *successful* trial's
  last unit is greened at **55.6 s**, against round 7's 86.7–131.9 s — the cap did not slow the arm that
  finished, and two of its three trials **stalled at the 900 s bound with nothing written at all**, a shape the
  pre-treatment rounds never showed. That stall, not the cap's arithmetic, is what makes half this batch
  unreadable, and nothing in this round separates it from host or model variance.
* Round 7's paired reading (H10's family, the *time to green under no cap*) stays the **last confirmed**
  reading on this question, unchanged: this round does not reinterpret it, and 3 of 6 trials here are
  unreadable rather than contradicted.
* H2's *success* form is untouched: this round is about *when* the units are green, not about whether they are
  (the frozen checks decide that, and the two arms' `checks_ok` status is 1/3 against 2/3, read by
  `analyze.py` as "too few samples"). The end-to-end gate stays as round 5 left it.
* One model, one task, three pairs, half of them unreadable: nothing here generalises, and this round opens no
  other. Its durable finding is the instrument: a 2048-token response ceiling truncates a team arm's tool
  arguments (a spawned member's grant call arrives empty and `issue_grant` refuses it), 4096 does not, and a
  ceiling experiment on this harness needs a bound that a capped arm can actually clear.
