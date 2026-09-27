# REPORT

Six independent deliverables. No test/acceptance file was modified; only the
implementation modules below were changed.

## 1. `units/measure/measure.py` — `Impl.to_cm(value, unit)`

Changed file: `units/measure/measure.py`.

Semantics:
- `"m"` → `value * 100`, `"cm"` → `value * 1`, `"mm"` → `value * 0.1`.
- Result is `int(round(...))`, so floats are accepted (`1.5 m → 150`,
  `20 mm → 2`) and float noise is rounded to the nearest integer centimetre.
- Any other unit (e.g. `"ft"`) raises `ValueError` before conversion.

Command run and result:

```
$ cd units/measure && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.02s
```

Additional direct check: `to_cm(1.5,'m')==150`, `to_cm(3,'cm')==3`,
`to_cm(20,'mm')==2`, `to_cm(1,'ft')` raised `ValueError`.

## 2. `units/slugify/slugify.py` — `Impl.slugify(text)`

Changed file: `units/slugify/slugify.py`.

Semantics:
- `str(text).lower()` first.
- `re.sub(r"[^a-z0-9]+", "-", ...)` collapses any run of characters that are not
  ASCII lowercase letters/digits into a single `-`.
- `.strip("-")` removes leading/trailing dashes.
- Empty result returns the literal `"untitled"`.

Boundary note: non-ASCII letters such as `é` are treated as separators
(`"Café Déjà Vu" → "caf-d-j-vu"`), consistent with the ASCII `[a-z0-9]` rule.

Command run and result:

```
$ cd units/slugify && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

## 3. `units/ranges/ranges.py` — `Impl.merge(ranges)`

Changed file: `units/ranges/ranges.py`.

Semantics:
- Each input pair is normalised to `(min, max)` (closes/degenerates handled).
- Intervals are sorted by start.
- Two intervals merge when `start <= current_end`, i.e. when they overlap or
  touch at a shared endpoint (`[1,3]` + `[3,5] → [1,5]`).
- Gap-of-one integer intervals are **not** merged, matching the frozen spec:
  `[1,4]` and `[5,7]` stay separate.
- Returns a list of tuples sorted by start; `[] → []`, `[[1,1]] → [(1,1)]`.

Note: the first draft merged gap-of-one (`start <= end+1`) and failed the frozen
test; it was corrected so "adjacent" means endpoint-touching only.

Command run and result:

```
$ cd units/ranges && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

## 4. `units/alpha/alpha.py` — `add`

Changed file: `units/alpha/alpha.py`. `check.py` was not modified.
Bug: `add` returned `a - b`; fixed to return `a + b`.

Command run and result:

```
$ cd units/alpha && python3 check.py
alpha ok
```

## 5. `units/beta/beta.py` — `mul`

Changed file: `units/beta/beta.py`. `check.py` was not modified.
Bug: `mul` returned `a + b`; fixed to return `a * b`.

Command run and result:

```
$ cd units/beta && python3 check.py
beta ok
```

## 6. `units/chain/stage1..4` — four-stage pipeline

Changed files: `units/chain/stage1.py`, `stage2.py`, `stage3.py`, `stage4.py`.
Nothing under `units/chain/tests/` was modified.

Semantics:
- `stage1.parse_orders(path)`: reads the CSV with `csv.DictReader`, returns a
  list of `{"id": int, "qty": int, "price": int}` in file order.
- `stage2.filter_orders(rows)`: keeps rows with `qty > 0` (strictly positive).
- `stage3.total_by_price(rows)`: sums `qty` grouped by `price`.
- `stage4.report(totals)`: one `"<price>:<qty>\n"` line per price, ordered by
  ascending price; no trailing blank line beyond the final `\n`.

End-to-end check on `data/orders.csv`:
`[{id:1,qty:2,price:10},{id:2,qty:0,price:5},{id:3,qty:3,price:7},{id:4,qty:1,price:7}]`
→ filter → `{10:2, 7:4}` → `report` → `'7:4\n10:2\n'`.

Command run and result:

```
$ cd units/chain && python3 -m pytest -q tests/
....                                                                     [100%]
4 passed in 0.12s
```

## Full acceptance sweep

```
== measure == 1 passed
== slugify == 1 passed
== ranges  == 1 passed
== alpha   == alpha ok
== beta    == beta ok
== chain   == 4 passed
```

All six acceptance commands pass (`ALL_ACCEPTANCE_PASS`). No acceptance/test
file was edited.
