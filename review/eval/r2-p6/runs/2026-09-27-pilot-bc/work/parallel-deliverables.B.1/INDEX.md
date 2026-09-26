# INDEX

Two independent deliverables, each with its own test suite.

## 1. `csvfix/` — CSV parsing and amount totaling

`csvfix/impl.py` provides:
- `parse(line)`: splits a CSV line, trims surrounding whitespace on every
  field, and returns the three fields as a list. Returns `None` when the line
  does not have exactly three columns.
- `total(rows)`: sums the third (amount) column, skipping blank amount fields;
  rows with fewer than three columns contribute nothing.

Verification command and result:

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.02s
```

## 2. `rules/` — rule evaluation engine

`rules/engine.py` implements `evaluate(rules, facts)`:
- `{"all": [name, ...]}` → `True` only when every listed fact is truthy.
- `{"any": [name, ...]}` → `True` when at least one listed fact is truthy.
- Unknown/empty rule shapes → `False`.

Verification command and result:

```
$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.02s
```

The test files `csvfix/test_csvfix.py` and `rules/test_rules.py` were not
modified; only the implementation modules were changed.
