# REPORT — six independent deliverables

Date: 2026-09-24. Each unit has its own frozen test file; **no test file was modified**.
Work was split into 6 independent parts, each assigned to one worker; the lead only
integrated and independently re-ran/verified everything below.

## Verification commands actually run (lead, from workspace root)

```
for d in csvfix rules ledger schedule intervals flags; do
  echo "===== $d ====="; (cd units/$d && python3 -m pytest -q .); echo "EXIT=$?"; done
```

Result:

```
===== csvfix =====   2 passed in 0.02s   EXIT=0
===== rules =====     1 passed in 0.01s   EXIT=0
===== ledger =====    2 passed in 0.01s   EXIT=0
===== schedule =====  2 passed in 0.02s   EXIT=0
===== intervals ===== 7 passed in 0.02s   EXIT=0
===== flags =====     8 passed in 0.02s   EXIT=0
```

Total: 22 passed, 0 failed, every suite EXIT=0.

Frozen test files (sha256, unchanged from the versions read before any edit):

```
588db2b78e9d1365dd8d7563abe70812bf958955665a95ad02b1e6a16c793724  units/csvfix/test_csvfix.py
07edf2bc2445551aa24d467a4ed09baca7cff1b8e76aa0760453440b0e521960  units/flags/test_flags.py
3c5fa1bb8a23b1d96acd8df686bce8cd73eb428e6ca6548279f8c3a1b96e0580  units/intervals/test_intervals.py
249141060049bcdd4c266e0448a139d5f8367d3969491c005ccdb5e4082747c8  units/ledger/test_ledger.py
4c425638c58c34872b0800f5fe19092c011785e123c15a1c69e2e99d762a5af8  units/rules/test_rules.py
d0eac968ccaa16dd71ca63f9a264f9aca424a03de2e94c6d77e607d7a5585b14  units/schedule/test_schedule.py
```

---

## 1. units/csvfix/ — `impl.py` (`parse`, `total`)

Files changed: `units/csvfix/impl.py`.

Semantics implemented:
- `parse(line)` parses one CSV record with the stdlib `csv` reader, so **quoted fields
  (including embedded commas) are handled**; every field is `.strip()`-ed of surrounding
  whitespace. It returns the 3-field list only when there are **exactly 3 columns**,
  otherwise `None`. `None`/empty input → `None` (also accepts an already-split list/tuple).
- `total(rows)` sums the **third column (amount)** as an int. Rows whose amount is blank/
  whitespace, and rows with fewer than 3 columns, are skipped; the running sum starts at 0.

Boundary cases probed:
- `parse('"a,b", 2 , x ')` → `['a,b', '2', 'x']`; `parse('a,2')` → `None`; `parse('')` → `None`.
- `total([['a','1','2'],['b','2',''],['c','9']])` → `2` (blank skipped, short row skipped).
- A non-integer third column (`'x'`) propagates `ValueError` (only blank is tolerated).

## 2. units/rules/ — `engine.evaluate`

Files changed: `units/rules/engine.py`.

Semantics implemented:
- `{"all": [...]}` → `True` only if every listed key maps to a truthy fact.
- `{"any": [...]}` → `True` if at least one listed key maps to a truthy fact.
- Returns a real `bool` (the tests use `is True` / `is False`).
- Missing keys are treated as falsy (`facts.get(key)`); a non-dict rule raises `TypeError`
  and a dict without `all`/`any` raises `ValueError`.

Boundary cases probed:
- `evaluate({"all": []}, {})` → `True` (vacuous truth); `evaluate({"any": []}, {})` → `False`.
- `evaluate({"all": ["z"]}, {"a": True})` → `False` (absent key is falsy).

## 3. units/ledger/ — `Ledger.apply`, `balance`

Files changed: `units/ledger/ledger.py`.

Semantics implemented:
- `Ledger.apply(rows, op)` returns the rows belonging to account `op`, **preserving original
  order**. Any row with a **negative amount raises `ValueError`** (checked across all rows
  before filtering). `kind` is `"deposit"` / `"withdraw"`.
- `balance(rows, account)`: deposits add, withdrawals subtract; an account with no rows → `0`.

Boundary cases probed:
- Interleaved accounts filtered/ordered correctly: `apply([deposit a 1, deposit b 2, withdraw a 1], "a")`
  → `[deposit a 1, withdraw a 1]`.
- `balance(rows, unknown)` → `0`; `apply([deposit a -1], "a")` raises `ValueError`.

## 4. units/schedule/ — `slots`, `overlaps`

Files changed: `units/schedule/schedule.py`.

Semantics implemented:
- `slots(ranges, minutes)`: sorts by `(lo, hi)`, then merges a following interval into the
  current one while the **gap `next_lo - prev_hi < minutes`** (strictly less than). Merged
  result is ascending; empty input → `[]`. `hi` uses `max` so nested intervals don't shrink.
- `overlaps(a, b)`: strict closed-interval overlap `a.lo < b.hi and b.lo < a.hi`, so
  **touching endpoints (e.g. `(0,10)` and `(10,20)`) do not overlap**.

Boundary cases probed:
- `slots([(0,60),(30,90),(120,150)], 30)` → `[(0,90),(120,150)]`; single interval preserved.
- Gap exactly equal to `minutes` is **not** merged: `slots([(0,60),(90,120)], 30)` →
  `[(0,60),(90,120)]`.
- `overlaps((0,10),(10,20))` → `False`; `overlaps((0,10),(5,20))` → `True`.

## 5. units/intervals/ — `Impl.merge`, `subtract`, `total_length`

Files changed: `units/intervals/intervals.py`.

Semantics implemented:
- `merge(ranges, gap=0)`: sorts by `lo`; joins when the **number of missing integers
  `next.lo - prev.hi - 1 <= gap`**; extends only when the new `hi` grows; nested/backwards
  inputs normalise; empty → `[]`.
- `subtract(ranges, hole)`: first normalises `ranges` via `merge`, then removes the closed
  `hole`. Disjoint segments are kept, fully covered segments vanish, and a segment spanning
  the hole splits into `(lo, hole_lo-1)` and `(hole_hi+1, hi)` (empty halves omitted).
- `total_length(ranges)`: merges first so overlaps count once, then sums `hi - lo + 1`
  (closed intervals, endpoints inclusive).

Boundary cases probed:
- `merge([[1,2],[4,5]])` → `[(1,2),(4,5)]`; with `gap=1` → `[(1,5)]` (exactly 1 missing int);
  adjacent `[[1,2],[3,4]]` → `[(1,4)]`; nested `[[0,10],[2,3]]` → `[(0,10)]`.
- `subtract([[0,10]], (4,6))` → `[(0,3),(7,10)]`; full cover → `[]`; touching hole
  `subtract([[0,4]], (4,9))` → `[(0,3)]`; disjoint → unchanged.
- `total_length([[0,2],[2,4]])` → `5` (overlap counted once); `[[3,3]]` → `1`.

## 6. units/flags/ — `Impl.parse`

Files changed: `units/flags/flags.py`.

Semantics implemented:
- Non-`-`-prefixed items **and the literal `"-"`** go to `positional` in order.
- `--` terminates option parsing: everything after it is positional and `--` itself is dropped.
- `--key=value` and `--key value` produce value pairs; `values[key]` is always a **list**,
  repeated keys accumulate in occurrence order.
- `--key` with no usable value (end of argv, or next token starts with `--`) becomes a switch.
- Bundled short `-abc` sets three switches `a`, `b`, `c`. Repeated switches stay `True`.
- Value tokens may themselves look like switches (`--k -5` → value `"-5"`; `--k=--v` → `"--v"`).

Boundary cases probed:
- `parse(["a","-","b"])["positional"]` → `['a','-','b']`.
- `parse(["--","--not-a-flag","x"])` → positional `['--not-a-flag','x']`, flags `{}`.
- `parse(["--k","-5"])` → values `{'k':['-5']}`; `parse(["--k=--v"])` → values `{'k':['--v']}`.
- `parse([])` → `{"values": {}, "flags": {}, "positional": []}`.

---

## Team split

One worker per independent unit (6 tasks, all settled): `worker-csvfix`, `worker-rules`,
`worker-ledger`, `worker-schedule`, `worker-intervals`, `worker-flags`. The lead performed
the integration step that cannot be split: cross-checking frozen-test hashes, re-running all
six suites, probing boundaries, and writing this report.
