# REPORT — twelve independent deliverables

All 12 units were implemented independently (one worker per unit) and then re-verified
by the integrator from the workspace root. No test/check/data file was modified
(their contents are byte-identical to the originals as shown at the end). The only
files edited are the 15 implementation files listed below.

## File changes

| # | Unit | Edited file(s) | Acceptance command | Verified result |
|---|------|----------------|--------------------|-----------------|
| 1 | csvfix | `units/csvfix/impl.py` | `cd units/csvfix && python3 -m pytest -q` | 2 passed, exit 0 |
| 2 | rules | `units/rules/engine.py` | `cd units/rules && python3 -m pytest -q` | 1 passed, exit 0 |
| 3 | ledger | `units/ledger/ledger.py` | `cd units/ledger && python3 -m pytest -q` | 2 passed, exit 0 |
| 4 | schedule | `units/schedule/schedule.py` | `cd units/schedule && python3 -m pytest -q` | 2 passed, exit 0 |
| 5 | intervals | `units/intervals/intervals.py` | `cd units/intervals && python3 -m pytest -q` | 7 passed, exit 0 |
| 6 | flags | `units/flags/flags.py` | `cd units/flags && python3 -m pytest -q` | 8 passed, exit 0 |
| 7 | measure | `units/measure/measure.py` | `cd units/measure && python3 -m pytest -q` | 1 passed, exit 0 |
| 8 | slugify | `units/slugify/slugify.py` | `cd units/slugify && python3 -m pytest -q` | 1 passed, exit 0 |
| 9 | ranges | `units/ranges/ranges.py` | `cd units/ranges && python3 -m pytest -q` | 1 passed, exit 0 |
| 10 | alpha | `units/alpha/alpha.py` | `cd units/alpha && python3 check.py` | `alpha ok`, exit 0 |
| 11 | beta | `units/beta/beta.py` | `cd units/beta && python3 check.py` | `beta ok`, exit 0 |
| 12 | chain | `units/chain/stage1.py` … `stage4.py` | `cd units/chain && python3 -m pytest -q tests/` | 4 passed, exit 0 |

## Per-unit semantics and boundaries

### 1. csvfix (`impl.py`)
- `parse(line)`: feeds the line through `csv.reader`, so quoted fields (including
  embedded commas) are parsed correctly; **strips** surrounding whitespace from every
  field; returns `None` unless the record has exactly 3 columns, else the 3 trimmed
  strings. Verified: `parse(' a , 2 , x ') == ["a","2","x"]`, `parse('a,2') is None`,
  `parse('"a,b",2,x') == ['a,b','2','x']`.
- `total(rows)`: amount is the **third** column (index 2). Skips rows that are `None`,
  not exactly 3 fields, or whose amount is empty/blank; a non-empty non-numeric amount
  raises `ValueError`. Verified: `total([["a","1","2"],["b","2",""]]) == 2`.
- Boundary/ambiguity: the module docstring calls "treating the third column as the
  amount" a bug, but the frozen test requires index 2 (index 1 would give 3, not 2).
  The test is authoritative and was kept passing.

### 2. rules (`engine.py`)
- `{"all":[...]}` → True iff **every** named key is present and truthy.
- `{"any":[...]}` → True iff **at least one** named key is present and truthy.
- Empty `"all"` → True (vacuous); empty `"any"` → False.
- Missing keys are falsy; a rule naming neither key is False; a rule containing both
  requires both. Always returns a real `bool` (safe for `is True/False`).

### 3. ledger (`ledger.py`)
- `Ledger.apply(rows, account)`: validates every row's kind (`deposit`/`withdraw`, else
  `ValueError`) and rejects negative cents (`ValueError`), then returns a **new** list of
  rows matching the account in original order (input not mutated).
- `balance(rows, account)`: deposits minus withdrawals for that account; 0 when absent.
- Boundaries: validation covers all input rows (not only the queried account); account
  matching is `==`; `balance` ignores other-account rows.

### 4. schedule (`schedule.py`)
- `slots(ranges, minutes)`: sorts by `lo`, merges when the gap `next_lo - prev_hi` is
  **strictly less** than `minutes`, extends `hi` with `max`; empty → `[]`.
  `minutes` equal to the gap keeps the segments apart.
- `overlaps(a,b)`: `a_lo < b_hi and b_lo < a_hi` — touching endpoints do **not** overlap.

### 5. intervals (`intervals.py`)
- `merge(ranges, gap=0)`: sorts by lo (normalizing `lo>hi`), joins when the number of
  **missing integers** `next.lo - prev.hi - 1 <= gap`; nested segments absorbed.
- `subtract(ranges, hole)`: normalizes input via `merge(ranges,0)` first, then drops
  covered segments, keeps disjoint ones, splits straddling ones; hole is closed
  (touching endpoints remove the shared integer).
- `total_length(ranges)`: union length `sum(hi-lo+1)` over merged input; `[]` → 0.

### 6. flags (`flags.py`)
- Non-`-` items and exactly `-` → `positional` in order.
- Bare `--` terminates options; everything after it (excluding `--`) is positional.
- `--key=value` and `--key value` accumulate into `values[key]` (always a list, in order).
- `--key` with no usable value (end of argv, or next item starting with `--`) → `True` flag.
- `-abc` → flags `a`,`b`,`c`. Repeated switches stay `True`; a key may appear in both
  `values` and `flags`. Empty argv → empty structure.
- Boundary: only a following `--`-prefixed item blocks value capture, so `--k -5` stores
  `"-5"` as a value; a bare `-5` standalone is treated as bundled short switch `5`.

### 7. measure (`measure.py`)
- `to_cm(value, unit)` factor table `{"m":100, "cm":1, "mm":0.1}`; unsupported/`None`/
  unhashable unit raises `ValueError`. Exact-match, case-sensitive keys; `mm` returns a
  float (`2.0 == 2`). No value-type validation (non-numeric → `TypeError`).

### 8. slugify (`slugify.py`)
- Lowercases; collapses each run of non-`[a-z0-9]` to a single `-`; strips leading/
  trailing `-`; empty result → `"untitled"`.

### 9. ranges (`ranges.py`)
- `merge(ranges)`: sorts `[lo,hi]` pairs and merges when `lo <= prev_hi` (overlapping or
  touching closed intervals); returns `(lo,hi)` tuples ascending by start; empty → `[]`.

### 10. alpha (`alpha.py`)
- `add(a,b)` now returns `a + b` (was `a - b`) → `check.py` prints `alpha ok`.

### 11. beta (`beta.py`)
- `mul(a,b)` now returns `a * b` (was `a + b`) → `check.py` prints `beta ok`.

### 12. chain (`stage1.py`–`stage4.py`)
- `stage1.parse_orders(path)`: `csv.reader`, skips header and blank rows, returns
  `{"id":int,"qty":int,"price":int}` in file order; path used relative to cwd.
- `stage2.filter_orders(rows)`: keeps `qty > 0`, preserving order/content.
- `stage3.total_by_price(rows)`: maps `price → sum(qty)`.
- `stage4.report(totals)`: `"price:qty\n"` lines sorted by price ascending (trailing newline).

## Commands actually run and results

All commands below were executed from the workspace root
`.../work/twelve-deliverables.D.2`; each exited 0.

```
cd units/csvfix    && python3 -m pytest -q        # 2 passed in 0.01s
cd units/rules     && python3 -m pytest -q        # 1 passed in 0.01s
cd units/ledger    && python3 -m pytest -q        # 2 passed in 0.01s
cd units/schedule  && python3 -m pytest -q        # 2 passed in 0.03s
cd units/intervals && python3 -m pytest -q        # 7 passed in 0.03s
cd units/flags     && python3 -m pytest -q        # 8 passed in 0.03s
cd units/measure   && python3 -m pytest -q        # 1 passed in 0.01s
cd units/slugify   && python3 -m pytest -q        # 1 passed in 0.01s
cd units/ranges    && python3 -m pytest -q        # 1 passed in 0.01s
cd units/alpha     && python3 check.py            # alpha ok
cd units/beta      && python3 check.py            # beta ok
cd units/chain     && python3 -m pytest -q tests/ # 4 passed in 0.03s
```

Additional integration sanity checks run (all matching expectations):
`csvfix.parse('"a,b",2,x') == ['a,b','2','x']`, `csvfix.parse('a,b,c,d') is None`,
`csvfix.total([["a","1","zz"]])` raises `ValueError`,
`flags.parse(["--k","-5"]) == {"values":{"k":["-5"]},"flags":{},"positional":[]}`,
`intervals.merge([[0,10],[2,3]]) == [(0,10)]`,
`intervals.subtract([[0,10]],(4,6)) == [(0,3),(7,10)]`,
`intervals.total_length([[0,2],[2,4]]) == 5`,
`schedule.slots([(0,60),(30,90),(120,150)],30) == [(0,90),(120,150)]`,
`schedule.overlaps((0,10),(10,20)) is False`,
`rules.evaluate({"all":[]},{}) is True`, `rules.evaluate({"any":[]},{}) is False`.

## Frozen files untouched
`units/**/test_*.py`, `units/alpha/check.py`, `units/beta/check.py`,
`units/chain/tests/*.py` and `units/chain/data/orders.csv` were re-read after the work
and match their original contents; only implementation files were edited.
