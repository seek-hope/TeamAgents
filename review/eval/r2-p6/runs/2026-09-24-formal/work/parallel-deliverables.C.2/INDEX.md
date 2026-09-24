# Deliverables

Two independent deliverables in this workspace. Each has its own test suite
which must pass without modifying the tests.

## 1. `csvfix/` — CSV parsing and totaling

`csvfix/impl.py` provides two functions used by `csvfix/test_csvfix.py`:

- `parse(line)` — splits one CSV line into fields, **trims whitespace from
  every field**, and **returns `None` when the column count is not exactly 3**.
  Quoted fields are parsed with the standard `csv` module so quoted commas do
  not split a row.
- `total(rows)` — sums the **third column (the amount)** across rows,
  **skipping blank amount fields** and ignoring rows with fewer than 3 columns.

## 2. `rules/` — rule engine

`rules/engine.py` implements `evaluate(rules, facts)`:

- `{"all": [...]}` → `True` only when every named fact is truthy.
- `{"any": [...]}` → `True` when at least one named fact is truthy.
- Missing facts are treated as falsy; an unknown/empty rule set returns `False`.

## How this was verified

Commands were run from the workspace root, each inside its own directory:

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s

$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

Both suites exit with status 0. The test files (`csvfix/test_csvfix.py`,
`rules/test_rules.py`) were not modified.
