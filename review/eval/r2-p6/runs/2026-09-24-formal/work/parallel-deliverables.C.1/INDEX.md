# INDEX

Two independent deliverables, each fixed in place with its test file left untouched.

## 1. `csvfix/` — CSV parsing and amount total

`impl.py` had a buggy `parse` (no trimming, no column validation) and `total`
(treated blank amounts as 0). Fixed semantics:

- `parse(line)`: strips each comma-separated field; returns `None` when the line
  does not have exactly 3 columns.
- `total(rows)`: sums the integer value of the **third** column, skipping rows
  whose amount field is blank or missing.

Test file `csvfix/test_csvfix.py` was **not** modified (sha256
`588db2b78e9d1365dd8d7563abe70812bf958955665a95ad02b1e6a16c793724`).

## 2. `rules/` — rule engine

`engine.evaluate(rules, facts)` was a stub returning `False`. Implemented:

- `{"all": [...]}` → `True` only when every named fact is truthy.
- `{"any": [...]}` → `True` when at least one named fact is truthy.
- Missing facts count as `False`; the return value is always a real `bool`.

Test file `rules/test_rules.py` was **not** modified (sha256
`4c425638c58c34872b0800f5fe19092c011785e123c15a1c69e2e99d762a5af8`).

## Verification

Commands actually run (from the workspace root):

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s

$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

Both exit statuses were `0`. The `sha256sum` of both test files after the runs
matched their pre-edit values, confirming the tests were not altered.
