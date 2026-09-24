# INDEX

Two independent deliverables in this workspace.

## 1. `csvfix/` — CSV parsing/total helper
- `impl.py` implements two functions:
  - `parse(line)`: splits a line on commas, trims whitespace from every field, and
    returns exactly 3 fields; returns `None` when the column count is not 3.
  - `total(rows)`: sums the third-column amount of each row as an integer,
    skipping rows with fewer than 3 columns or a blank/whitespace amount.
- `test_csvfix.py` encodes the expected semantics and was **not modified**.

Verify:
```
cd csvfix && python3 -m pytest -q
# ..  -> 2 passed in 0.00s
```

## 2. `rules/` — rule evaluation engine
- `engine.py` implements `evaluate(rules, facts)`:
  - `{"all": [...]}` → `True` only when every referenced fact is truthy.
  - `{"any": [...]}` → `True` when at least one referenced fact is truthy.
  - Missing facts are treated as false; unknown rule shapes return `False`.
- `test_rules.py` encodes the expected semantics and was **not modified**.

Verify:
```
cd rules && python3 -m pytest -q
# .  -> 1 passed in 0.00s
```

Both runs were executed from the workspace root as
`(cd csvfix && python3 -m pytest -q)` and `(cd rules && python3 -m pytest -q)`;
both exited with status 0.
