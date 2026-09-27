# REPORT — six independent deliverables

Date: 2026-09-24. No test/check/acceptance file was modified; only the
implementation files listed below were written.

## Final verification (all executed from the workspace root)

| # | Block | Command | Result |
|---|-------|---------|--------|
| 1 | units/measure | `cd units/measure && python3 -m pytest -q` | `1 passed` (exit 0) |
| 2 | units/slugify | `cd units/slugify && python3 -m pytest -q` | `1 passed` (exit 0) |
| 3 | units/ranges  | `cd units/ranges && python3 -m pytest -q` | `1 passed` (exit 0) |
| 4 | units/alpha   | `cd units/alpha && python3 check.py` | `alpha ok` (exit 0) |
| 5 | units/beta    | `cd units/beta && python3 check.py` | `beta ok` (exit 0) |
| 6 | units/chain   | `cd units/chain && python3 -m pytest -q tests/` | `4 passed` (exit 0) |

All six were run in one consolidated pass; each returned exit code 0.

## Files changed

- `units/measure/measure.py` — implemented `Impl.to_cm`
- `units/slugify/slugify.py` — implemented `Impl.slugify`
- `units/ranges/ranges.py` — implemented `Impl.merge`
- `units/alpha/alpha.py` — fixed `add` (`a - b` -> `a + b`)
- `units/beta/beta.py` — fixed `mul` (`a + b` -> `a * b`)
- `units/chain/stage1.py` … `stage4.py` — implemented `parse_orders`,
  `filter_orders`, `total_by_price`, `report`
- `REPORT.md` — this file

## Per-block semantics and boundaries

### 1. measure — `Impl.to_cm(value, unit)`
- Supported units and factors to cm: `m` ×100, `cm` ×1, `mm` ×0.1.
- Result is an `int` via `int(round(value * factor))`, so fractional inputs are
  supported (`1.5 m -> 150`, `20 mm -> 2`) and float noise is rounded away.
- Any unit not in `{m, cm, mm}` raises `ValueError`.
- Boundary: valid units are matched case-sensitively; `"M"` is rejected.

### 2. slugify — `Impl.slugify(text)`
- Lowercases, replaces every maximal run of non-`[a-z0-9]` characters with a
  single `-`, strips leading/trailing `-`; an empty result returns `untitled`.
- Boundary: the allowed alphabet is ASCII alphanumeric only, so non-ASCII
  letters are separators (e.g. `"Café Déjà-Vu 42" -> "caf-d-j-vu-42"`), and
  non-string input is not coerced.

### 3. ranges — `Impl.merge(ranges)`
- Accepts an iterable of `[start, end]` closed intervals, normalises each pair
  with `min`/`max`, sorts by start, and merges when the next start is `<=` the
  current end. Returns a list of `(start, end)` tuples sorted by start.
- Semantic boundary (important): merging happens only for overlapping intervals
  or intervals that share an endpoint. The frozen test requires
  `merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]`, i.e. `(1,4)` and `(5,7)` stay
  separate. Therefore "adjacent" here means sharing a common point
  (`[1,3],[3,5] -> [(1,5)]`), not merely being consecutive by one
  (`[1,3],[4,6] -> [(1,3),(4,6)]`).
- `merge([])` returns `[]`; a single point `[1,1]` returns `[(1,1)]`.

### 4. alpha — `add`
- `add(a, b)` now returns `a + b`; `check.py` unchanged and passes.

### 5. beta — `mul`
- `mul(a, b)` now returns `a * b`; `check.py` unchanged and passes.

### 6. chain
- `stage1.parse_orders(path)`: reads the CSV at `path` with `csv.DictReader`
  and returns a list of `{"id": int, "qty": int, "price": int}` dicts in file
  order. Boundary: values are cast with `int()`; malformed rows would raise.
- `stage2.filter_orders(rows)`: keeps rows with `qty > 0`, preserving order,
  returning new dict objects filtered (original list unchanged).
- `stage3.total_by_price(rows)`: sums `qty` grouped by `price`, returning a
  `{price: total_qty}` dict. Boundary: empty input gives `{}`; sums assume
  numeric `qty`.
- `stage4.report(totals)`: formats one `"price:qty\n"` line per key, sorted by
  price ascending, e.g. `{10:2, 7:4} -> "7:4\n10:2\n"`. Boundary: empty dict
  gives `""`.

## Edge-case evidence actually run

```
$ python3 -c "from units.measure.measure import Impl ..."
1.5m-> 150 int
3cm-> 3
20mm-> 2
ft raises ValueError: unknown unit: 'ft'
slug: 'hello-world' 'a-b' 'untitled'
slug unicode: 'caf-d-j-vu-42'

$ python3 -c "from units.ranges.ranges import Impl ..."
merge([[1,3],[3,5]])       -> [(1, 5)]
merge([[1,3],[4,6]])       -> [(1, 3), (4, 6)]
merge([[1,3],[2,4],[5,7]]) -> [(1, 4), (5, 7)]
merge([])                  -> []
```
