# REPORT — twelve independent deliverables

All 12 units were implemented/fixed independently. No test or acceptance file
(`test_*.py`, `check.py`) was modified — verified byte-for-byte against the
original fixture with `diff -q` (all 15 files `UNCHANGED`).

Verification run once more from scratch by the integrator over every unit
(all commands real, all exit 0):

```
cd csvfix    && python3 -m pytest -q          -> 2 passed
cd rules     && python3 -m pytest -q          -> 1 passed
cd ledger    && python3 -m pytest -q          -> 2 passed
cd schedule  && python3 -m pytest -q          -> 2 passed
cd intervals && python3 -m pytest -q          -> 7 passed
cd flags     && python3 -m pytest -q          -> 8 passed
cd measure   && python3 -m pytest -q          -> 1 passed
cd slugify   && python3 -m pytest -q          -> 1 passed
cd ranges    && python3 -m pytest -q          -> 1 passed
cd alpha     && python3 check.py              -> prints "alpha ok", exit 0
cd beta      && python3 check.py              -> prints "beta ok", exit 0
cd chain     && python3 -m pytest -q tests/   -> 4 passed
```

Per-unit semantics actually implemented:

1. **csvfix/impl.py** — `parse(line)`: splits on `,`, strips each field, returns
   `None` unless there are exactly 3 fields, else the 3 trimmed strings.
   `total(rows)`: sums the third column as `int`, silently skipping rows with a
   blank/absent third field. Boundary: no quote handling beyond plain split;
   field count `!= 3` is the only validation; blank amounts are excluded, not
   counted as 0.
2. **rules/engine.py** — `evaluate(rules, facts)`: `"all"` → `all(facts.get(n))`,
   `"any"` → `any(facts.get(n))`; returns a real `bool` (satisfies `is True` /
   `is False`); missing names are falsy; neither key present raises `ValueError`.
3. **ledger/ledger.py** — `Ledger.apply(rows, op)`: raises `ValueError` if any
   row amount is negative, otherwise returns rows with `account == op` in
   original order. `balance(rows, account)`: `sum(deposit) - sum(withdraw)`,
   ignores other accounts, returns `0` when none match.
4. **schedule/schedule.py** — `slots(ranges, minutes)`: sorts by start and merges
   when the gap `next_lo - prev_hi` is **strictly** `< minutes`; gap `== minutes`
   stays split; empty → `[]`; tuples returned. `overlaps(a, b)`: strict
   `a_lo < b_hi and b_lo < a_hi`, so touching endpoints are not an overlap.
5. **intervals/intervals.py** — `merge(ranges, gap=0)`: normalizes reversed
   endpoints, sorts by `(lo, hi)`, merges when missing integers
   `next.lo - prev.hi - 1 <= gap`, extends `hi` with `max` (nested handled);
   returns tuples ascending. `subtract(ranges, hole)`: normalizes the hole,
   merges ranges first, keeps disjoint parts, drops covered parts, splits
   spanning parts into two. `total_length`: sums `hi-lo+1` over merged segments
   (overlap counted once).
6. **flags/flags.py** — `Impl.parse(argv)`: one pass producing
   `{"values": {k: [..]}, "flags": {k: True}, "positional": [..]}`. Items not
   starting with `-`, or exactly `-`, are positional; `--` drops itself and makes
   all remaining items positional; `--k=v` and `--k v` both set values; a
   `--k` followed by end-of-argv or another `--…` item becomes a flag (so `-5` is
   still accepted as a value); `-abc` sets flags `a,b,c`; repeated values
   accumulate in order, repeated switches stay `True`.
7. **measure/measure.py** — `Impl.to_cm(value, unit)`: `m`→`value*100`,
   `cm`→`value`, `mm`→`value/10`; any other unit raises `ValueError`.
8. **slugify/slugify.py** — `Impl.slugify(text)`: lowercases, collapses runs of
   `[^a-z0-9]` to a single `-`, strips leading/trailing `-`, empty → `"untitled"`.
9. **ranges/ranges.py** — `Impl.merge(ranges)`: sorts by start, merges when
   `lo <= current_hi` (overlapping **and** touching), returns tuples sorted by
   start; empty → `[]`.
10. **alpha/alpha.py** — `add` fixed from `a - b` to `a + b`.
11. **beta/beta.py** — `mul` fixed from `a + b` to `a * b`.
12. **chain/** — `stage1.parse_orders(path)`: `csv.DictReader`, int-valued
    `{"id","qty","price"}` dicts in file order. `stage2.filter_orders(rows)`:
    keep `qty > 0`, order preserved. `stage3.total_by_price(rows)`:
    `{price: sum(qty)}`. `stage4.report(totals)`: `"price:total"` lines sorted by
    price ascending, each terminated by `\n` (trailing newline included).

## Real command transcript

```
$ cd units/csvfix    && python3 -m pytest -q          # 2 passed
$ cd units/rules     && python3 -m pytest -q          # 1 passed
$ cd units/ledger    && python3 -m pytest -q          # 2 passed
$ cd units/schedule  && python3 -m pytest -q          # 2 passed
$ cd units/intervals && python3 -m pytest -q          # 7 passed
$ cd units/flags     && python3 -m pytest -q          # 8 passed
$ cd units/measure   && python3 -m pytest -q          # 1 passed
$ cd units/slugify   && python3 -m pytest -q          # 1 passed
$ cd units/ranges    && python3 -m pytest -q          # 1 passed
$ cd units/alpha     && python3 check.py              # alpha ok
$ cd units/beta      && python3 check.py              # beta ok
$ cd units/chain     && python3 -m pytest -q tests/   # 4 passed
$ diff -q <fixture>/<test-or-check-file> <work>/<file>  # all 15 UNCHANGED
```

Integrator re-run summary: `OVERALL_FAIL=0`.

## Notes / boundaries not covered by the frozen tests

- `flags.parse` has no defined behaviour for a single-dash long form with
  `=` (e.g. `-k=v`); the implementation treats it as a bundle of switches.
- `intervals` and `ranges` only guarantee integer closed intervals; inputs
  `lo > hi` are normalized in `intervals` (reversed) and sorted-then-merged in
  `ranges`.
- `slugify` is ASCII (`a-z0-9`); non-ASCII letters are treated as separators.
