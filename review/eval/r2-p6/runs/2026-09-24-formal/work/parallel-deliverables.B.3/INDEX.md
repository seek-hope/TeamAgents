# Deliverables

Two independent deliverables live in this workspace.

## 1. `csvfix/` — CSV parsing/total fixes

`impl.py` implements the semantics expected by `test_csvfix.py`:

- `parse(line)` splits on commas, strips each field, and returns `None`
  when the line does not have exactly 3 columns.
- `total(rows)` sums the third column (the amount), skipping rows whose
  amount field is blank or missing.

## 2. `rules/` — rule engine

`engine.evaluate(rules, facts)` implements two rule shapes:

- `{"all": [...]}` → `True` when every named fact is truthy.
- `{"any": [...]}` → `True` when at least one named fact is truthy.

Unknown rules evaluate to `False`.

## Verification

Tests were not modified. Real commands and results:

```
$ cd csvfix && python3 -m pytest -q
2 passed in 0.00s

$ cd rules && python3 -m pytest -q
1 passed in 0.00s
```

Both runs exited with status 0.
