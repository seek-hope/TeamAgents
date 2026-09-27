# INDEX — parallel deliverables D.1

Two independent deliverables, each fixed by a separate worker and verified by the leader.

## 1. `csvfix/` — CSV parsing and summing

- **What it is:** `csvfix/impl.py` provides `parse(line)` and `total(rows)`.
  - `parse(line)`: splits on `,`, trims whitespace from every field, and returns
    `None` when the column count is not exactly 3.
  - `total(rows)`: sums the amount column (index 2), skipping rows with a blank
    (or absent) amount field.
- **Bug fixed:** the original `parse` did a raw `line.split(",")` with no trimming
  and no column validation; `total` mishandled blanks and used the wrong column.
- **Test file:** `csvfix/test_csvfix.py` — not modified
  (sha256 `588db2b78e9d1365dd8d7563abe70812bf958955665a95ad02b1e6a16c793724`).
- **Verify:** `cd csvfix && python3 -m pytest -q` → `2 passed in 0.01s` (exit 0).

## 2. `rules/` — rule engine

- **What it is:** `rules/engine.py` implements `evaluate(rules, facts)`, supporting
  `{"all": [names...]}` (True iff every named fact is truthy) and
  `{"any": [names...]}` (True iff at least one named fact is truthy). Missing
  facts are falsy; the result is always a real `bool`.
- **Bug fixed:** the original `evaluate` was a stub that always returned `False`.
- **Test file:** `rules/test_rules.py` — not modified
  (sha256 `4c425638c58c34872b0800f5fe19092c011785e123c15a1c69e2e99d762a5af8`).
- **Verify:** `cd rules && python3 -m pytest -q` → `1 passed in 0.01s` (exit 0).

## How verification was done

From the workspace root:

```
cd csvfix && python3 -m pytest -q   # ..      [100%] 2 passed in 0.01s  (exit 0)
cd rules  && python3 -m pytest -q   # .       [100%] 1 passed in 0.01s  (exit 0)
```

Environment: Python 3.13.15, pytest 9.0.3. Each part was handled by its own
worker instance with the corresponding pytest command as its acceptance check;
the leader re-ran both suites and compared the test-file checksums against the
originals to confirm no test file was touched.
