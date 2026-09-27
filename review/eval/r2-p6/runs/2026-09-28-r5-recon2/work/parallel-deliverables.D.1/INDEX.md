# Deliverables Index

Two independent deliverables, each self-verified with its own real test command.

## 1. `csvfix/` — CSV parsing/total helpers

- **What it is:** `csvfix/impl.py` provides `parse(line)` and `total(rows)`.
  `parse` splits on commas, trims whitespace from every field, and returns
  `None` when the column count is not exactly 3. `total` sums the third
  column (amount) as an integer, skipping rows with fewer than 3 fields and
  rows whose amount field is blank.
- **Changed file:** `csvfix/impl.py` (implementation only).
- **Test file:** `csvfix/test_csvfix.py` — left untouched.

## 2. `rules/` — Rule evaluator

- **What it is:** `rules/engine.py` implements `evaluate(rules, facts)` for
  `{"all": [...]}` (True iff every listed key is truthy in `facts`) and
  `{"any": [...]}` (True iff at least one listed key is truthy). Missing keys
  count as falsy.
- **Changed file:** `rules/engine.py` (implementation only).
- **Test file:** `rules/test_rules.py` — left untouched.

## How this was verified

Real commands run from the workspace root, with real results:

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.02s            # exit code 0

$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s            # exit code 0
```

Both test suites pass, and neither test file was modified.
