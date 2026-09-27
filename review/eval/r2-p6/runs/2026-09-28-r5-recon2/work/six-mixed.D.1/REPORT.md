# REPORT — six independent deliverables (`work/six-mixed.D.1`)

Date: 2026-09-24
Verification model: one worker per part implemented the change; the lead re-ran every
acceptance command itself and probed the semantics boundaries. No test or acceptance
file was modified (`units/**/test_*.py`, `units/alpha/check.py`, `units/beta/check.py`,
`units/chain/tests/*`, `units/chain/data/orders.csv`).

## Global verification command (real run)

```
cd units/measure && python3 -m pytest -q ;   # 1 passed
cd ../slugify  && python3 -m pytest -q ;     # 1 passed
cd ../ranges   && python3 -m pytest -q ;     # 1 passed
cd ../alpha    && python3 check.py ;         # alpha ok
cd ../beta     && python3 check.py ;         # beta ok
cd ../chain    && python3 -m pytest -q tests/ ; # 4 passed
```
All six exited 0.

---

## 1. `units/measure/` — `Impl.to_cm(value, unit)`

File changed: `units/measure/measure.py` (only).

Semantics boundary:
- Supported units map to centimeters-per-unit: `m`→100, `cm`→1, `mm`→0.1.
- `value` may be `int` or `float`; result is an `int` via `int(round(value * factor))`.
- Any other unit (e.g. `ft`) raises `ValueError` (with the offending unit in the message).
- Boundary: rounding is Python `round` (banker's rounding) applied after multiplication;
  the frozen test only requires the exact cases `1.5 m→150`, `3 cm→3`, `20 mm→2`.

Real command / result:
```
cd units/measure && python3 -m pytest -q
# 1 passed in 0.01s   (exit 0)
```
Extra probe:
```
to_cm(1.5,"m")==150 ; to_cm(3,"cm")==3 ; to_cm(20,"mm")==2
to_cm(1,"ft") -> ValueError: unsupported unit: 'ft'
```

## 2. `units/slugify/` — `Impl.slugify(text)`

File changed: `units/slugify/slugify.py` (only).

Semantics boundary:
- Input is converted with `str(text).lower()`.
- Each maximal run of characters outside `[a-z0-9]` is replaced by one `-`.
- Leading/trailing `-` are stripped; an empty result becomes `"untitled"`.
- Boundary: only ASCII lowercase letters/digits survive as literal characters (non-ASCII
  letters are treated as separators). Runs collapse to a single hyphen, never multiple.

Real command / result:
```
cd units/slugify && python3 -m pytest -q
# 1 passed in 0.02s   (exit 0)
```
Extra probe:
```
"Hello, World!" -> 'hello-world' ; "  --A@@B-- " -> 'a-b' ; "***" -> 'untitled'
```

## 3. `units/ranges/` — `Impl.merge(ranges)`

File changed: `units/ranges/ranges.py` (only).

Semantics boundary:
- Closed integer intervals; sorted by start, result is a list of `(start, end)` tuples.
- Two intervals are merged when they **overlap** (`next_start <= last_end`), including
  full containment (`[[1,10],[3,4]] -> [(1,10)]`).
- **Adjacency note:** the task prose said "adjacent or overlapping", but the frozen
  acceptance test requires `[[5,7],[1,3],[2,4]] == [(1,4),(5,7)]`, i.e. the adjacent
  closed intervals `[1,4]` and `[5,7]` must stay separate. The frozen test is
  authoritative here, so the implementation merges overlap only. Merging adjacency
  (`start <= last_end + 1`) was tried and fails the frozen test with `[(1,7)]`.
- Empty input → `[]`; single point `[[1,1]] -> [(1,1)]`.

Real command / result:
```
cd units/ranges && python3 -m pytest -q
# 1 passed in 0.02s   (exit 0)
```
Extra probe:
```
merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]
merge([]) == [] ; merge([[1,1]]) == [(1,1)]
merge([[1,2],[3,4]]) == [(1,2),(3,4)]   # adjacent NOT merged (per frozen test)
merge([[1,10],[3,4]]) == [(1,10)]        # containment merged
```

## 4. `units/alpha/` — bug fix

File changed: `units/alpha/alpha.py` (only); `check.py` untouched.
- `add` returned `a - b`; corrected to `a + b`.

Real command / result:
```
cd units/alpha && python3 check.py
# alpha ok   (exit 0)
```

## 5. `units/beta/` — bug fix

File changed: `units/beta/beta.py` (only); `check.py` untouched.
- `mul` returned `a + b`; corrected to `a * b`.

Real command / result:
```
cd units/beta && python3 check.py
# beta ok   (exit 0)
```

## 6. `units/chain/` — `stage1`..`stage4`

Files changed: `units/chain/stage1.py`, `stage2.py`, `stage3.py`, `stage4.py` (only);
`units/chain/tests/*` and `data/orders.csv` untouched.

Semantics boundary (each stage is a pure function):
- `stage1.parse_orders(path)`: CSV with header `id,qty,price`; returns one dict per data
  row in file order with `id`/`qty`/`price` coerced to `int`.
- `stage2.filter_orders(rows)`: keeps rows with `qty > 0` (drops `qty == 0`; the pricing
  of a dropped row never enters later totals).
- `stage3.total_by_price(rows)`: dict `price -> sum(qty)` for all rows sharing a price.
- `stage4.report(totals)`: `"price:total\n"` lines sorted by price ascending, trailing
  newline included.
- End-to-end on the shipped data: rows with ids 2 (qty 0) dropped; `{10:2, 7:4}`;
  report `"7:4\n10:2\n"`.

Real command / result:
```
cd units/chain && python3 -m pytest -q tests/
# 4 passed in 0.09s   (exit 0)
```
Extra end-to-end probe:
```
parse_orders("chain/data/orders.csv")
 -> [{'id':1,'qty':2,'price':10},{'id':2,'qty':0,'price':5},
     {'id':3,'qty':3,'price':7},{'id':4,'qty':1,'price':7}]
filter_orders(...) -> ids 1,3,4
total_by_price(...) -> {10: 2, 7: 4}
report(...) -> '7:4\n10:2\n'
```

---

## Files changed (summary)

```
units/measure/measure.py     implemented to_cm
units/slugify/slugify.py     implemented slugify
units/ranges/ranges.py       implemented merge
units/alpha/alpha.py         fixed add (a-b -> a+b)
units/beta/beta.py           fixed mul (a+b -> a*b)
units/chain/stage1.py        implemented parse_orders
units/chain/stage2.py        implemented filter_orders
units/chain/stage3.py        implemented total_by_price
units/chain/stage4.py        implemented report
REPORT.md                    this report
```
No frozen test/check/data file was touched.
