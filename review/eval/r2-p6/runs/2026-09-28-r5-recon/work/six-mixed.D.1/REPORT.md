# REPORT — six independent deliverables

All six deliverables were implemented independently. Frozen test/acceptance files
were not modified (verified: the repository's `git status` shows no changes under
`units/`, and the frozen files' current SHA-256 hashes are recorded below).

Each block below states the implemented semantics boundary, the exact command run,
and the real observed result. Every command listed was actually executed from the
workspace root `/home/rimuru/.../six-mixed.D.1`.

## 1. `units/measure/` — `Impl.to_cm(value, unit)`
- File changed: `units/measure/measure.py` (only).
- Semantics: factor map `{m: 100, cm: 1, mm: 0.1}`; unsupported unit raises
  `ValueError` (KeyError is caught and re-raised). Result is `int(round(value * factor))`.
- Boundary/decisions: only exact-unit strings `"m"`, `"cm"`, `"mm"` are accepted;
  any other unit (including `None`, `"M"`, `"ft"`) raises `ValueError`.
  Conversion rounds to the nearest integer, so fractional centimetres round rather
  than truncate (e.g. `2.55 m -> 255 cm`).
- Command: `cd units/measure && python3 -m pytest -q`
- Result: `1 passed in 0.01s`, exit code 0.
- Extra spot check: `to_cm(1.5,"m")==150`, `to_cm(3,"cm")==3`, `to_cm(20,"mm")==2`,
  `to_cm(1,"ft")` raises `ValueError: unsupported unit: 'ft'`.

## 2. `units/slugify/` — `Impl.slugify(text)`
- File changed: `units/slugify/slugify.py` (only).
- Semantics: `None` becomes `""`; then lowercase; each maximal run of characters
  outside `[a-z0-9]` becomes a single `-`; leading/trailing `-` stripped; empty
  result returns `"untitled"`.
- Boundary/decisions: "alphanumeric" is ASCII `a-z0-9` only (after lowercasing),
  so `_` and non-ASCII letters are treated as separators (`"A__b" -> "a-b"`).
- Command: `cd units/slugify && python3 -m pytest -q`
- Result: `1 passed in 0.01s`, exit code 0.
- Extra spot check: `"Hello, World!" -> "hello-world"`, `"  --A@@B-- " -> "a-b"`,
  `"***" -> "untitled"`.

## 3. `units/ranges/` — `Impl.merge(ranges)`
- File changed: `units/ranges/ranges.py` (only).
- Semantics: intervals treated as **closed** `[start, end]`; sorted by start; a current
  interval and the next are merged when `next_start <= current_end` (overlap or
  touching at an endpoint); result is a list of `(start, end)` tuples sorted by start.
  Empty input returns `[]`; `[[1,1]]` returns `[(1,1)]`.
- Boundary/decisions (important): "adjacent" is interpreted in the continuous
  closed-interval sense — intervals that **share an endpoint** merge
  (`[1,2] + [2,3] -> [(1,3)]`), while intervals separated by a real gap do **not**
  (`[1,2] + [3,4] -> [(1,2),(3,4)]`, since the open gap `(2,3)` is uncovered).
  This is exactly what the frozen test requires: `[[5,7],[1,3],[2,4]] == [(1,4),(5,7)]`,
  where `[1,4]` and `[5,7]` are separated by the gap `(4,5)` and correctly stay apart.
  (An earlier delegation wording of mine suggested merging integer-consecutive
  intervals `[1,2]+[3,4] -> [1,4]`; that was inconsistent with the frozen test and was
  rejected in favour of the continuous closed-interval rule above.)
- Command: `cd units/ranges && python3 -m pytest -q`
- Result: `1 passed in 0.01s`, exit code 0.
- Extra spot checks: `[[1,2],[2,3],[5,5]] -> [(1,3),(5,5)]`,
  `[[1,2],[3,4]] -> [(1,2),(3,4)]`, `[[10,12],[1,20],[0,0]] -> [(0,0),(1,20)]`,
  `[] -> []`, `[[1,1]] -> [(1,1)]`.

## 4. `units/alpha/` — fix `alpha.py`
- File changed: `units/alpha/alpha.py` (only; `check.py` untouched).
- Bug: `add` returned `a - b`; fixed to `return a + b`.
- Command: `cd units/alpha && python3 check.py`
- Result: prints `alpha ok`, exit code 0.

## 5. `units/beta/` — fix `beta.py`
- File changed: `units/beta/beta.py` (only; `check.py` untouched).
- Bug: `mul` returned `a + b`; fixed to `return a * b`.
- Command: `cd units/beta && python3 check.py`
- Result: prints `beta ok`, exit code 0.

## 6. `units/chain/` — `stage1`..`stage4`
- Files changed: `units/chain/stage1.py`, `stage2.py`, `stage3.py`, `stage4.py` (only;
  nothing under `units/chain/tests/` was modified).
- Semantics:
  - `stage1.parse_orders(path)`: `csv.DictReader` over `data/orders.csv` (header
    `id,qty,price`), returning dicts with **int** `id`/`qty`/`price` in file order.
  - `stage2.filter_orders(rows)`: keep rows with `qty > 0`.
  - `stage3.total_by_price(rows)`: dict `price -> sum(qty)` over the given rows.
  - `stage4.report(totals)`: string of `"price:total\n"` lines sorted ascending by price.
- Boundary/decisions: `qty == 0` is excluded; integer indices used for grouping and
  sorting, so output line order is numeric by price.
- Command: `cd units/chain && python3 -m pytest -q tests/`
- Result: `4 passed in 0.01s`, exit code 0.

## Cross-check: frozen files unmodified
`git status --porcelain` (run from the workspace root) lists only unrelated
`review/eval/...` paths — no `units/` file appears. Current hashes of the frozen
acceptance files:
- `units/measure/test_measure.py` — `4d1ca7a8eb9de778485e1a098269aebae5763096bcf87f2b9a9480a5268d766d`
- `units/slugify/test_slugify.py` — `fc56ef2fd6b0007cca58e323e841dc6b75be168d1a08ead13905ace1d134c9fd`
- `units/ranges/test_ranges.py` — `4efde602ab3325b7e35c7c59b29a48739f705fa33a335c8eb4d2f11d82507a62`
- `units/alpha/check.py` — `8f30e265cbfa6ae5fdcf42d313033cd960d83bd1d7cd7f8f7ba3ca09eb289dd4`
- `units/beta/check.py` — `4e7ac3aa4b4ddcc08f0e4902f7e25de1af457b7cbcce051aad1394b80ca373e1`
- `units/chain/tests/test_stage1.py` — `13aa797e9aef3c9aba1c20677f6fec223964e6c285658c224dae79e3efdc3670`
- `units/chain/tests/test_stage2.py` — `b48a335be723c00645c2305619266096635a4c2de66703e2987af54a4ece56e5`
- `units/chain/tests/test_stage3.py` — `ed5c68b1370162f168d99306dc6cd7feca0d8668757ed0acb7d97a57b130ad32`
- `units/chain/tests/test_stage4.py` — `c3d2971eca0f4e523fd9a27a4d017ccc7797dbaf8b8abeffdd411c300fe8c453`
