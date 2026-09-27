# Deliverables

Two independent, self-contained deliverables in this workspace:

## 1. `csvfix/` — CSV parsing / totalling fixes

`csvfix/impl.py` provides two functions whose semantics are expressed by
`csvfix/test_csvfix.py` (unchanged):

- `parse(line)`: splits on `,`, trims whitespace from every field, and
  returns `None` when the line does not have exactly 3 columns.
- `total(rows)`: sums the **third** column as the amount, skipping rows
  with fewer than 3 columns and blank/whitespace-only amount fields.

### Verification

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.01s
```

## 2. `rules/` — rule evaluation engine

`rules/engine.py` implements `evaluate(rules, facts)` as required by
`rules/test_rules.py` (unchanged):

- `{"all": [names...]}` → `True` only when every named fact is truthy.
- `{"any": [names...]}` → `True` when at least one named fact is truthy.
- Unknown facts count as falsy; an unrecognised rule shape returns `False`.

### Verification

```
$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.02s
```

## Summary of commands actually run

| Command | Result |
| --- | --- |
| `cd csvfix && python3 -m pytest -q` | 2 passed |
| `cd rules && python3 -m pytest -q` | 1 passed |
