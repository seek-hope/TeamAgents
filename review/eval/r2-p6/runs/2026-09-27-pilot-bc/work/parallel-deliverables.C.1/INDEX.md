# INDEX

Two independent deliverables in this workspace.

## 1. `csvfix/` — CSV parsing / amount totaling

- `csvfix/impl.py`:
  - `parse(line)` parses one CSV line into **exactly 3 trimmed fields** (quote-aware,
    so a quoted field may contain a comma). Returns **`None`** when the column count
    is not 3 (or the input is not a string / malformed).
  - `total(rows)` sums the **third column (amount)** and **skips blank amount fields**
    (also skips rows that are missing the amount column).
- `csvfix/test_csvfix.py` — unchanged; expresses the expected semantics.

## 2. `rules/` — rule engine

- `rules/engine.py`:
  - `evaluate(rules, facts)` supports `{"all": [...]}` (every listed fact truthy) and
    `{"any": [...]}` (at least one listed fact truthy). Empty `all` is vacuously True,
    empty `any` is False, and unknown rule shapes evaluate to `False`.
- `rules/test_rules.py` — unchanged.

## Verification

Run from this workspace root (real commands and results):

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.01s

$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

Both test files were left untouched; only `csvfix/impl.py` and `rules/engine.py` were edited.
