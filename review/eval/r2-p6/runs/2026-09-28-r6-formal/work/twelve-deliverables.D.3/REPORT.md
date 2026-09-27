# Twelve Deliverables — Implementation Report

Date: 2026-09-24
Workspace: `twelve-deliverables.D.3`

Twelve mutually independent units were fixed/implemented. Frozen acceptance
files (`test_*.py`, `check.py`, `tests/`) were **not** modified.

Team model: one worker instance per unit (`w-csvfix`, `w-rules`, `w-ledger`,
`w-schedule`, `w-intervals`, `w-flags`, `w-measure`, `w-slugify`, `w-ranges`,
`w-alpha`, `w-beta`, `w-chain`), each given its own acceptance command. After
all workers settled, the orchestrator independently re-ran every acceptance
command (see "Verification" below).

---

## 1. `units/csvfix/` — `impl.parse` / `impl.total`

Implementation (`units/csvfix/impl.py`):
- `parse(line)`: splits on `,`, strips whitespace from every field. Returns the
  list of 3 strings only if the split yields **exactly 3** columns; otherwise
  returns `None`.
- `total(rows)`: iterates rows; ignores any row whose length is not 3; skips a
  blank (or whitespace-only) third field; otherwise adds `int(row[2])`. Returns
  `0` for empty input.

Semantic boundaries:
- Quote-aware CSV (RFC 4180) is **not** implemented; splitting is a plain
  `str.split(",")`. The frozen tests do not require quoting.
- A non-numeric third field raises `ValueError` (not covered by tests).
- Negative amounts are summed, not rejected.
- `total` assumes each row is a sequence; only length-3 rows are considered.

## 2. `units/rules/` — `engine.evaluate(rules, facts)`

Implementation (`units/rules/engine.py`):
- `{"all": [names]}` → `True` iff every named fact is truthy (missing name = falsy).
- `{"any": [names]}` → `True` iff at least one named fact is truthy.
- Returns a genuine `bool` (tests use `is True` / `is False`).

Semantic boundaries:
- If a rule contains both `"all"` and `"any"`, `"all"` takes precedence.
- `{"all": []}` is `True`; `{"any": []}` is `False` (Python built-ins).
- A rule with neither key returns `False`.
- `facts=None` is treated as `{}`. Truthiness (not `is True`) decides a fact.

## 3. `units/ledger/` — `Ledger.apply` / `balance`

Implementation (`units/ledger/ledger.py`):
- `Ledger.apply(rows, op)`: keeps rows whose account equals `op`, preserving
  order; raises `ValueError` if a kept row has a negative amount.
- `balance(rows, account)`: `deposit` adds, `withdraw` subtracts; returns `0`
  when there are no rows for the account.

Semantic boundaries:
- Negative amounts on rows of *other* accounts are not validated (filtering
  happens before the check).
- `balance` raises `ValueError` for an unknown `kind` on a matching account
  (defensive; not exercised by tests).
- Amounts are used as-is (assumed ints; no rounding or type check).

## 4. `units/schedule/` — `slots` / `overlaps`

Implementation (`units/schedule/schedule.py`):
- `slots(ranges, minutes)`: sorts by `(lo, hi)`, then merges when
  `next_lo - prev_hi < minutes`, extending with `max(prev_hi, next_hi)` (so
  nested ranges are swallowed). Empty input → `[]`.
- `overlaps(a, b)`: `a[0] < b[1] and b[0] < a[1]` — strictly overlapping.

Semantic boundaries:
- Gaps of exactly `minutes` are **not** merged; only gaps strictly smaller are.
- Touching closed intervals (`lo == prev_hi`) count as gap 0 and merge only if
  `minutes > 0`.
- `overlaps` returns a real `bool`; touching endpoints (`(0,10)` & `(10,20)`) do
  not overlap.
- Inputs are not mutated; results are tuples.

## 5. `units/intervals/` — `Impl.merge` / `subtract` / `total_length`

Implementation (`units/intervals/intervals.py`):
- `merge(ranges, gap=0)`: sorts by `(lo, hi)`, merges when the number of missing
  integers `next.lo - prev.hi - 1 <= gap`, extending to `max(hi)`. `[]` → `[]`.
- `subtract(ranges, hole)`: first normalizes with `merge(gap=0)`, then for each
  segment clips to `(lo, hlo-1)` and/or `(hhi+1, hi)`; disjoint segments are kept.
- `total_length(ranges)`: `sum(hi - lo + 1)` over `merge(ranges)` (overlaps
  counted once).

Semantic boundaries:
- `gap=0` merges overlapping **and** directly adjacent intervals.
- `subtract` uses closed intervals: a hole that ends at `lo-1` or starts at
  `hi+1` removes nothing.
- `total_length` counts integers (closed endpoints included, `(3,3)` → 1).

## 6. `units/flags/` — `Impl.parse(argv)`

Implementation (`units/flags/flags.py`):
- Positional: tokens not starting with `-`, plus the bare `"-"`.
- `"--"` terminates option parsing and is itself omitted; all later tokens are
  positional.
- `--key=value` always yields the value (even empty or `--`-prefixed).
- `--key value` yields a value only when a next token exists and does not start
  with `--` (so `-5` is accepted as a value); otherwise `key` becomes a switch.
- `-abc` expands to switches `a`, `b`, `c`.
- `values` entries are lists accumulating in order; repeated switches stay `True`.

Semantic boundaries:
- A single-dash cluster is always treated as switches (no `-k value` short-option
  form); e.g. a standalone `-5` would become switch `"5"` (it is consumed as a
  value when it follows `--key`).
- A `--key` followed by another `--...` token becomes a switch.
- `--key=` yields an empty-string value.
- `values`, `flags`, `positional` are always present in the result.

## 7. `units/measure/` — `Impl.to_cm`

Implementation (`units/measure/measure.py`):
- `"m"` → `value * 100`; `"cm"` → `value`; `"mm"` → `value / 10`.
- Any other unit raises `ValueError`.

Semantic boundaries:
- Units are case-sensitive; no prefixes/whitespace/aliases.
- `mm` uses true division, so results may be `float` (equal-comparable to ints).
- No validation of non-numeric `value`.

## 8. `units/slugify/` — `Impl.slugify`

Implementation (`units/slugify/slugify.py`):
- Lowercases; `re.sub(r"[^0-9a-zA-Z]+", "-", ...)`; strips leading/trailing
  `-`; returns `"untitled"` when the result is empty.

Semantic boundaries:
- Only ASCII alphanumerics survive; all other characters (including Unicode
  letters and `_`) become separators, and runs collapse to one `-`.
- No transliteration; input must be a string.

## 9. `units/ranges/` — `Impl.merge`

Implementation (`units/ranges/ranges.py`):
- Sorts by `(start, end)`; merges when `start <= prev_end` (overlap or
  containment); returns a list of tuples sorted by start; `[]` → `[]`.

Semantic boundaries:
- **No adjacency merge**: `[1,4]` and `[5,7]` stay separate (directly touching
  disjoint intervals are not joined).
- Nested intervals are swallowed.

## 10. `units/alpha/` — `alpha.add`

- Fixed `add` to `return a + b` (was `a - b`). `check.py` untouched; prints `alpha ok`.

## 11. `units/beta/` — `beta.mul`

- Fixed `mul` to `return a * b` (was `a + b`). `check.py` untouched; prints `beta ok`.

## 12. `units/chain/` — `stage1`..`stage4`

Implementation (`units/chain/stage{1,2,3,4}.py`):
- `stage1.parse_orders(path)`: `csv.DictReader`; returns dicts with int `id`,
  `qty`, `price`.
- `stage2.filter_orders(rows)`: keeps rows with `qty != 0`, preserving order.
- `stage3.total_by_price(rows)`: sums `qty` grouped by `price` (int keys).
- `stage4.report(totals)`: `"price:total\n"` lines sorted ascending by price;
  `""` for empty input.

Semantic boundaries:
- `parse_orders` assumes the header `id,qty,price`; extra columns are ignored by
  the dict construction and malformed ints raise `ValueError`. A header-only file
  yields `[]`. `kwargs` are accepted for call compatibility.
- `filter_orders` treats a missing/`None` `qty` as `0` and therefore drops it.
- `total_by_price` returns an insertion-ordered dict, not sorted.
- `report` sorts numerically (integer keys), newline-terminated lines.

---

## Verification

Every acceptance command was re-run by the orchestrator from the shared
workspace root after all workers settled. Both per-unit `pytest -q` runs and the
two `check.py` runs exited 0:

| Unit | Command | Result |
| --- | --- | --- |
| csvfix | `cd units/csvfix && python3 -m pytest -q` | `2 passed in 0.01s` |
| rules | `cd units/rules && python3 -m pytest -q` | `1 passed in 0.01s` |
| ledger | `cd units/ledger && python3 -m pytest -q` | `2 passed in 0.01s` |
| schedule | `cd units/schedule && python3 -m pytest -q` | `2 passed in 0.01s` |
| intervals | `cd units/intervals && python3 -m pytest -q` | `7 passed in 0.02s` |
| flags | `cd units/flags && python3 -m pytest -q` | `8 passed in 0.01s` |
| measure | `cd units/measure && python3 -m pytest -q` | `1 passed in 0.01s` |
| slugify | `cd units/slugify && python3 -m pytest -q` | `1 passed in 0.01s` |
| ranges | `cd units/ranges && python3 -m pytest -q` | `1 passed in 0.01s` |
| chain | `cd units/chain && python3 -m pytest -q tests/` | `4 passed in 0.02s` |
| alpha | `cd units/alpha && python3 check.py` | `alpha ok` |
| beta | `cd units/beta && python3 check.py` | `beta ok` |

Files changed: `units/csvfix/impl.py`, `units/rules/engine.py`,
`units/ledger/ledger.py`, `units/schedule/schedule.py`,
`units/intervals/intervals.py`, `units/flags/flags.py`,
`units/measure/measure.py`, `units/slugify/slugify.py`, `units/ranges/ranges.py`,
`units/alpha/alpha.py`, `units/beta/beta.py`,
`units/chain/stage1.py`..`stage4.py`, plus this `REPORT.md`.

Frozen acceptance files were left byte-for-byte untouched (verified by
re-reading content; `check.py` and all `test_*.py` still contain their original
assertions).
