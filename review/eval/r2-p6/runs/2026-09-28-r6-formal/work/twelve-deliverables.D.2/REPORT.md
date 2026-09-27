# Twelve Independent Deliverables — Report

All 12 units were implemented independently. The acceptance/check files were **not**
modified. Every acceptance command below was executed by me (the integrator) in this
workspace on 2026-09-24; the full verbose transcript is in `VERIFY.log`.

- Python: 3.13.15, pytest 9.0.3
- Workspace: `.../twelve-deliverables.D.2`
- Frozen-file proof: all `test_*.py` / `check.py` files have mtimes strictly **before**
  the implementation writes (implementations written 06:06–06:07; acceptance files last
  touched 03:44 or earlier on a previous date). No acceptance file was touched.

## Per-unit semantics boundary and results

### 1. `units/csvfix/impl.py`
- `parse(line)`: splits on `,`, **strips** whitespace from every field, and returns the
  list only when the split yields **exactly 3 fields**; otherwise `None` (also `None`
  for non-string input). No CSV quoting dialect is handled.
- `total(rows)`: sums the third (amount) column; skips rows with `<3` fields, `None`/blank
  third fields, and non-numeric values (tries `int`, falls back to `float`); starts at 0.
- Command: `cd units/csvfix && python3 -m pytest -q` → `2 passed` (exit 0).

### 2. `units/rules/engine.py`
- `evaluate(rules, facts)` supports `{"all": [...]}` (every named fact truthy) and
  `{"any": [...]}` (at least one). If both keys exist, `all` wins.
- Missing facts count as false; empty lists use vacuous semantics (`all([])=True`,
  `any([])=False`); results are real `bool` values. Non-dict rules, unknown keys, or
  non-sequence operands return `False`.
- Command: `cd units/rules && python3 -m pytest -q` → `1 passed` (exit 0).

### 3. `units/ledger/ledger.py`
- `Ledger.apply(rows, op)`: validates every row first — `kind` must be `deposit`/`withdraw`,
  amount must be `>= 0` — raising `ValueError` otherwise. Returns the rows whose account
  equals `op`, in original order.
- `balance(rows, account)`: `sum(deposit) - sum(withdraw)` for that account; returns `0`
  when the account has no rows; unknown kinds raise `ValueError`.
- Command: `cd units/ledger && python3 -m pytest -q` → `2 passed` (exit 0).

### 4. `units/schedule/schedule.py`
- `slots(ranges, minutes)`: sorts by lower bound, merges the next range into the current
  segment when the gap `lo - prev_hi < minutes` (overlap/containment gives a negative gap
  and also merges), extending the end to `max(hi)`; gap exactly `== minutes` splits. Returns
  a list of tuples ascending; empty input → `[]`.
- `overlaps(a, b)`: `a[0] < b[1] and b[0] < a[1]` — endpoint-touching intervals such as
  `(0,10)`/`(10,20)` do **not** overlap, and zero-length intervals never overlap (this is
  half-open behavior, as required by the test even though the module docstring says close).
- Command: `cd units/schedule && python3 -m pytest -q` → `2 passed` (exit 0).

### 5. `units/intervals/intervals.py`
- `merge(ranges, gap=0)`: closed integer intervals. Sorts by lower bound and merges when
  the number of missing integers `next.lo - prev.hi - 1 <= gap`; nested/overlapping ranges
  collapse into the wider span. `gap=0` merges adjacent/overlapping; `gap=1` also bridges a
  one-integer hole. Returns tuples ascending; empty → `[]`.
- `subtract(ranges, hole)`: normalizes its input via `merge` first, keeps disjoint segments,
  drops fully covered ones, and splits a spanning segment into `(lo, h_lo-1)` and
  `(h_hi+1, hi)`.
- `total_length(ranges)`: `sum(hi - lo + 1)` over merged segments, so overlaps count once;
  `0` for empty input.
- Command: `cd units/intervals && python3 -m pytest -q` → `7 passed` (exit 0).

### 6. `units/flags/flags.py`
- `Impl.parse(argv)` returns `{"values": {...}, "flags": {...}, "positional": [...]}`.
  `--key=value` and `--key value` both produce key/value pairs; `--key` with no usable
  following value (end of argv, or next item starts with `--`) becomes a switch set to
  `True`. `-abc` expands to switches `a`,`b`,`c`. `--` is omitted and makes all remaining
  items positional (even `--`-prefixed ones). Non-dash items and lone `-` are positional.
  `values` entries are ordered lists, so repeated keys accumulate; repeated switches stay
  `True`.
- Boundary: a following item that starts with a single `-` (e.g. `-5`, `-abc`) is consumed
  as a value; only `--`-prefixed items block value consumption.
- Command: `cd units/flags && python3 -m pytest -q` → `8 passed` (exit 0).

### 7. `units/measure/measure.py`
- `Impl.to_cm(value, unit)`: `m`=100, `cm`=1, `mm`=0.1 centimetres; returns the float product.
  Unknown or unhashable units raise `ValueError`. Units are matched exactly (case-sensitive,
  no stripping), so `M` and `"m "` are illegal.
- Command: `cd units/measure && python3 -m pytest -q` → `1 passed` (exit 0).

### 8. `units/slugify/slugify.py`
- `Impl.slugify(text)`: `str(text).lower()`, maps each non-alphanumeric character to `-`
  (via `str.isalnum`, so Unicode letters/digits survive and `_` does not), collapses runs of
  `-`, strips leading/trailing `-`, and returns `"untitled"` when nothing remains.
- Command: `cd units/slugify && python3 -m pytest -q` → `1 passed` (exit 0).

### 9. `units/ranges/ranges.py`
- `Impl.merge(ranges)`: normalizes each pair to `(min, max)`, sorts by start, and merges when
  `start <= current_end` (**overlap only** — merely adjacent ranges stay separate). Returns a
  list of tuples sorted by start; empty → `[]`.
- Command: `cd units/ranges && python3 -m pytest -q` → `1 passed` (exit 0).

### 10. `units/alpha/alpha.py`
- Fixed `add` to return `a + b` (was `a - b`). `check.py` untouched.
- Command: `cd units/alpha && python3 check.py` → `alpha ok` (exit 0).

### 11. `units/beta/beta.py`
- Fixed `mul` to return `a * b` (was `a + b`). `check.py` untouched.
- Command: `cd units/beta && python3 check.py` → `beta ok` (exit 0).

### 12. `units/chain/`
- `stage1.parse_orders(path)`: reads the CSV with `csv.DictReader`, coerces `id`/`qty`/`price`
  to `int`, preserves row order.
- `stage2.filter_orders(rows)`: drops `qty == 0` rows, preserves order.
- `stage3.total_by_price(rows)`: sums `qty` per distinct `price`.
- `stage4.report(totals)`: renders one `price:total` line per entry plus a trailing newline,
  ordered by price ascending. End-to-end on the shipped data: `"7:4\n10:2\n"`.
- Command: `cd units/chain && python3 -m pytest -q tests/` → `4 passed` (exit 0).

## Verification summary (run by the integrator)

| # | Command | Result |
|---|---------|--------|
| 1 | `cd units/csvfix && python3 -m pytest -q` | 2 passed, exit 0 |
| 2 | `cd units/rules && python3 -m pytest -q` | 1 passed, exit 0 |
| 3 | `cd units/ledger && python3 -m pytest -q` | 2 passed, exit 0 |
| 4 | `cd units/schedule && python3 -m pytest -q` | 2 passed, exit 0 |
| 5 | `cd units/intervals && python3 -m pytest -q` | 7 passed, exit 0 |
| 6 | `cd units/flags && python3 -m pytest -q` | 8 passed, exit 0 |
| 7 | `cd units/measure && python3 -m pytest -q` | 1 passed, exit 0 |
| 8 | `cd units/slugify && python3 -m pytest -q` | 1 passed, exit 0 |
| 9 | `cd units/ranges && python3 -m pytest -q` | 1 passed, exit 0 |
| 10 | `cd units/alpha && python3 check.py` | `alpha ok`, exit 0 |
| 11 | `cd units/beta && python3 check.py` | `beta ok`, exit 0 |
| 12 | `cd units/chain && python3 -m pytest -q tests/` | 4 passed, exit 0 |

Total: 29 pytest tests + 2 checker scripts, all passing. Detailed per-test PASSED lines are
in `VERIFY.log`.

## Files changed

- `units/csvfix/impl.py`, `units/rules/engine.py`, `units/ledger/ledger.py`,
  `units/schedule/schedule.py`, `units/intervals/intervals.py`, `units/flags/flags.py`,
  `units/measure/measure.py`, `units/slugify/slugify.py`, `units/ranges/ranges.py`,
  `units/alpha/alpha.py`, `units/beta/beta.py`,
  `units/chain/stage1.py`, `units/chain/stage2.py`, `units/chain/stage3.py`,
  `units/chain/stage4.py`
- No test/acceptance file was modified (`units/**/test_*.py`, `units/alpha/check.py`,
  `units/beta/check.py`, `units/chain/tests/**`).
- Artifacts added by test runs only: `__pycache__/`, `.pytest_cache/` (removed in cleanup).

## Team / process

Each of the 12 units was delegated to a separate worker instance (`w01_csvfix` …
`w12_chain`), one per independent part, each given the unit's own acceptance command and
the instruction not to touch frozen files. All 12 settled SUCCEEDED. The integrator then
re-ran every acceptance command independently (table above) and wrote this report. No
cross-unit dependency exists, and no two workers edited the same file.
