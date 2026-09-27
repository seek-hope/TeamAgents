# REPORT — twelve independent deliverables

Date: 2026-09-28. Workspace: `twelve-deliverables.D.3`.
No test file and no `check.py` was modified; each acceptance command was re-run by the
integrator after all workers finished. All 12 commands exit 0.

## Summary table

| # | Unit | Files changed | Acceptance command | Result |
|---|------|---------------|--------------------|--------|
| 1 | csvfix | `units/csvfix/impl.py` | `cd units/csvfix && python3 -m pytest -q` | 2 passed, EXIT=0 |
| 2 | rules | `units/rules/engine.py` | `cd units/rules && python3 -m pytest -q` | 1 passed, EXIT=0 |
| 3 | ledger | `units/ledger/ledger.py` | `cd units/ledger && python3 -m pytest -q` | 2 passed, EXIT=0 |
| 4 | schedule | `units/schedule/schedule.py` | `cd units/schedule && python3 -m pytest -q` | 2 passed, EXIT=0 |
| 5 | intervals | `units/intervals/intervals.py` | `cd units/intervals && python3 -m pytest -q` | 7 passed, EXIT=0 |
| 6 | flags | `units/flags/flags.py` | `cd units/flags && python3 -m pytest -q` | 8 passed, EXIT=0 |
| 7 | measure | `units/measure/measure.py` | `cd units/measure && python3 -m pytest -q` | 1 passed, EXIT=0 |
| 8 | slugify | `units/slugify/slugify.py` | `cd units/slugify && python3 -m pytest -q` | 1 passed, EXIT=0 |
| 9 | ranges | `units/ranges/ranges.py` | `cd units/ranges && python3 -m pytest -q` | 1 passed, EXIT=0 |
| 10 | alpha | `units/alpha/alpha.py` | `cd units/alpha && python3 check.py` | `alpha ok`, EXIT=0 |
| 11 | beta | `units/beta/beta.py` | `cd units/beta && python3 check.py` | `beta ok`, EXIT=0 |
| 12 | chain | `units/chain/stage1.py` … `stage4.py` | `cd units/chain && python3 -m pytest -q tests/` | 4 passed, EXIT=0 |

All commands above were actually executed from this workspace root (12/12 EXIT=0).

## Per-unit semantics and boundaries

### 1. `units/csvfix/` — CSV row parsing and amount summation
`parse(line)`: parses one line with the standard `csv.reader` (so quoted commas work),
strips surrounding whitespace on each field, and returns exactly 3 fields. Returns
`None` if the line does not have exactly 3 columns, is `None`, or is not parseable.
Boundary: column count must be exactly 3; quoting is supported.
`total(rows)`: sums the third (amount) field as `int`. Blank/whitespace-only amounts,
`None` amounts, empty rows and rows with fewer than 3 fields are skipped (contribute 0),
not errors. Boundary: malformed row shapes are skipped rather than raising.

### 2. `units/rules/` — all/any rule evaluation
`evaluate({"all": [k...]}, facts)` is True iff every referenced fact is truthy
(empty list is vacuously True). `evaluate({"any": [k...]}, facts)` is True iff at least
one referenced fact is truthy (empty list is False). Facts are read by truthiness;
missing keys count as False rather than raising. Returns a real `bool`.
Boundary: non-dict `rules` raises `TypeError`; a rule with neither `all` nor `any`
raises `ValueError`; if both keys are present, `all` takes precedence.

### 3. `units/ledger/` — single-account ledger
`Ledger.apply(rows, op)`: first scans all `(kind, account, cents)` rows and raises
`ValueError` if any amount is negative, then returns only the rows whose account equals
`op`, preserving the original order (tuples returned unchanged). Boundary: a negative
amount for *any* account is rejected, not just for `op`.
`balance(rows, account)`: deposits add, withdrawals subtract, over rows of that account
only; returns `0` when no row matches. Unknown `kind` values contribute nothing.

### 4. `units/schedule/` — closed-interval slot merging
`slots(ranges, minutes)`: sorts by `(lo, hi)` and merges the next segment when the gap
`next_lo - prev_hi` is strictly less than `minutes` (keeping the larger `hi`); result is
ascending by start, empty input returns `[]`. Boundary: a gap exactly equal to `minutes`
does *not* merge; overlapping and adjacent-with-gap-0 segments do merge.
`overlaps(a, b)`: strict closed-interval intersection `a_lo < b_hi and b_lo < a_hi`, so
segments that merely touch at an endpoint (e.g. `(0,10)`/`(10,20)`) do not overlap.

### 5. `units/intervals/` — closed integer-interval algebra
`Impl.merge(ranges, gap=0)`: sorts by `lo`, and joins adjacent segments when the number
of missing integers `next_lo - prev_hi - 1` is `<= gap`; contained/nested ranges are
absorbed. Returns ascending `(lo, hi)` tuples; empty input `[]`.
`Impl.subtract(ranges, hole)`: normalizes via `merge` first, keeps segments disjoint from
the hole unchanged, drops fully covered segments, and splits straddling segments into
`(lo, hole_lo - 1)` and `(hole_hi + 1, hi)`.
`Impl.total_length(ranges)`: merges first, then sums `hi - lo + 1`, so overlaps are
counted once and endpoints are included.

### 6. `units/flags/` — command-line style parsing
`Impl.parse(argv)` returns `{"values": {k: [v...]}, "flags": {k: True}, "positional": [...]}`.
Tokens not starting with `-`, or exactly `-`, are positional in order. `--` is dropped and
everything after it is positional (even strings that look like flags). `--key=value` and
`--key value` both append to `values[key]` (list, in order). `--key` is a switch when
there is no usable following value: end of argv, or the next token starts with `--`.
A value may itself look like a switch (`--k -5` gives `["-5"]`; `--k=--v` gives `["--v"]`).
`-abc` expands to switches `a`, `b`, `c`; repeated switches stay `True`.

### 7. `units/measure/` — length conversion
`Impl.to_cm(value, unit)`: `m` → ×100, `cm` → ×1, `mm` → ×0.1. Unsupported units raise
`ValueError`. Boundary: comparison is exact on the unit string (no aliases, no
case-folding); a non-string/unhashable unit also raises `ValueError`. Return value is
numeric (`20 mm` → `2.0`, which equals `2`).

### 8. `units/slugify/` — slug generation
`Impl.slugify(text)`: lowercases, replaces each maximal run of non-alphanumeric
characters (`[^a-z0-9]+`) with a single `-`, strips leading/trailing `-`, and returns
`"untitled"` when the result is empty. Boundary: ASCII `[a-z0-9]` is the alphanumeric
set, so non-ASCII letters are also collapsed.

### 9. `units/ranges/` — closed-interval merging
`Impl.merge(ranges)`: sorts intervals by start and merges when `start <= last_end`
(so intervals touching at a single point merge), keeping the max end. Returns a list of
`(start, end)` tuples sorted by start; empty input returns `[]`.

### 10. `units/alpha/`
`alpha.add(a, b)` returned `a - b`; fixed to return `a + b`. `check.py` untouched.

### 11. `units/beta/`
`beta.mul(a, b)` returned `a + b`; fixed to return `a * b`. `check.py` untouched.

### 12. `units/chain/` — four-stage pipeline
`stage1.parse_orders(path)`: `csv.DictReader`, skips blank rows, returns file-ordered
`[{"id": int, "qty": int, "price": int}, ...]`; rows with `qty == 0` are kept here.
`stage2.filter_orders(rows)`: keeps `qty > 0`, preserving order.
`stage3.total_by_price(rows)`: aggregates `{price: total_qty}`.
`stage4.report(totals)`: renders `price:total` lines sorted by price ascending with a
trailing newline.
Real end-to-end on `data/orders.csv`:
`parse → [{id:1,qty:2,price:10},{id:2,qty:0,price:5},{id:3,qty:3,price:7},{id:4,qty:1,price:7}]`,
`filter → ids 1,3,4`, `totals → {10: 2, 7: 4}`, `report → '7:4\n10:2\n'`.

## Integrity evidence

- Test/acceptance files were left untouched. Their mtimes predate all worker edits
  (checks/tests: 2026-09-24 and 2026-09-28 03:44–03:46; edited implementations:
  2026-09-28 05:35). `check.py` sha256:
  `8f30e265cbfa6ae5fdcf42d313033cd960d83bd1d7cd7f8f7ba3ca09eb289dd4` (alpha),
  `4e7ac3aa4b4ddcc08f0e4902f7e25de1af457b7cbcce051aad1394b80ca373e1` (beta).
- Extra edge-case probes (beyond the frozen tests) were run and matched the documented
  boundaries above, e.g. `parse('"a,b", 2 , x ') == ['a,b','2','x']`,
  `parse('a,2,3,4') is None`, `evaluate({'all':['missing']}, {'a':1}) is False`,
  `Ledger.apply([('deposit','b',-1)], 'a')` raises `ValueError`,
  `Impl.merge([[0,10],[2,3]]) == [(0,10)]`,
  `Impl.subtract([[5,7],[1,3]], (2,6)) == [(1,1),(7,7)]`,
  `Impl.parse(['--','--x','-'])` puts `['--x','-']` in positional,
  `slugify('  --A@@B-- ') == 'a-b'`, `merge([[1,3],[3,5],[8,9]]) == [(1,5),(8,9)]`.
