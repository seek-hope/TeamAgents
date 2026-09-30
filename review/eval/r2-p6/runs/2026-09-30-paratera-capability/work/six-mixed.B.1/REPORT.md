# REPORT.md

Six independent deliverables, each verified by running its frozen acceptance command.
No test/acceptance file was modified (`check.py`, `test_*.py`, `tests/`, `orders.csv` untouched).

## 1. `units/measure/` — `Impl.to_cm(value, unit)`

**File changed:** `units/measure/measure.py`

**Semantics / boundaries**
- Supported units: `m` → ×100, `cm` → ×1, `mm` → ×0.1.
- Result is an **integer** number of centimetres (`int(round(...))`), so `1.5 m → 150`,
  `3 cm → 3`, `20 mm → 2`.
- Any unit other than `m`/`cm`/`mm` raises `ValueError` (checked before any arithmetic).

**Command & result**
```
$ cd units/measure && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

## 2. `units/slugify/` — `Impl.slugify(text)`

**File changed:** `units/slugify/slugify.py`

**Semantics / boundaries**
- Lowercases the input.
- Collapses one or more consecutive non-alphanumeric characters into a single `-`
  (regex `[\W_]+`, unicode-aware; underscore counts as a separator since it is not alphanumeric).
- Strips leading/trailing `-`.
- If the result is empty, returns the literal `untitled`.

**Command & result**
```
$ cd units/slugify && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

## 3. `units/ranges/` — `Impl.merge(ranges)`

**File changed:** `units/ranges/ranges.py`

**Semantics / boundaries**
- Closed integer intervals; output is a list of tuples sorted by start.
- Overlapping or **endpoint-touching** intervals merge (`start <= current_end`), e.g.
  `[1,3]`+`[2,4]` → `(1,4)`, and `[1,4]`+`[4,7]` → `(1,7)`.
- Intervals separated by a gap are kept separate: `[1,4]` and `[5,7]` stay
  `[(1,4),(5,7)]` (this is what the frozen test requires).
- Reversed pairs are normalised with `min`/`max`; nested intervals are absorbed.
- Empty input returns `[]`; a single interval is returned unchanged, e.g. `[[1,1]] → [(1,1)]`.

**Command & result**
```
$ cd units/ranges && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

Note: an initial implementation merged gap-of-1 intervals (`+1`); the frozen test
`merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]` failed, so the condition was corrected to
`start <= current_end`.

## 4. `units/alpha/` — `alpha.add`

**File changed:** `units/alpha/alpha.py`

**Semantics / boundaries**
- Bug was `return a - b`; fixed to `return a + b`.

**Command & result**
```
$ cd units/alpha && python3 check.py
alpha ok
```

## 5. `units/beta/` — `beta.mul`

**File changed:** `units/beta/beta.py`

**Semantics / boundaries**
- Bug was `return a + b`; fixed to `return a * b`.

**Command & result**
```
$ cd units/beta && python3 check.py
beta ok
```

## 6. `units/chain/` — stages 1–4

**Files changed:** `units/chain/stage1.py`, `stage2.py`, `stage3.py`, `stage4.py`

**Semantics / boundaries**
- `stage1.parse_orders(path)`: reads CSV with `csv.DictReader`, returns a list of dicts
  with `id`, `qty`, `price` cast to `int` (order preserved as in the file).
- `stage2.filter_orders(rows)`: keeps rows with `qty > 0` only.
- `stage3.total_by_price(rows)`: sums `qty` per distinct `price`, returns `{price: total_qty}`.
- `stage4.report(totals)`: emits `"<price>:<qty>\n"` lines sorted ascending by price,
  e.g. `{10:2, 7:4} → "7:4\n10:2\n"`.

**Command & result**
```
$ cd units/chain && python3 -m pytest -q tests/
....                                                                     [100%]
4 passed in 0.01s
```

## Extra sanity checks (run manually)

```
measure 1.55m -> 155
measure bad unit -> ValueError: unsupported unit: 'ft'
slugify 'A--B__C' -> a-b-c
slugify '' -> untitled
ranges touching [[1,4],[4,7]] -> [(1, 7)]
ranges nested [[1,10],[2,3]] -> [(1, 10)]
ranges unsorted-reversed [[7,5],[3,1]] -> [(1, 3), (5, 7)]
```

## Combined acceptance run

```
== measure ==  1 passed
== slugify ==  1 passed
== ranges  ==  1 passed
== alpha   ==  alpha ok
== beta    ==  beta ok
== chain   ==  4 passed
```
