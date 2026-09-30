# Index

Two independent deliverables, each with its own tests. Tests were **not** modified.

## 1. `csvfix/` — CSV parsing and amount totaling

`csvfix/impl.py` implements:

- `parse(line)` — splits on `,`, strips surrounding whitespace from every field,
  and returns `None` when the column count is not 3.
- `total(rows)` — sums the **third** column (the amount), skipping rows whose
  amount field is blank.

Verified with:

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s
```

## 2. `rules/` — rule evaluation engine

`rules/engine.py` implements `evaluate(rules, facts)`:

- `{"all": [...]}` → `True` only when every listed key is truthy in `facts`.
- `{"any": [...]}` → `True` when at least one listed key is truthy.
- Missing keys count as `False`; an unrecognised rule shape returns `False`.

Verified with:

```
$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

## Verification summary

| Deliverable | Command | Result |
|-------------|---------|--------|
| csvfix      | `cd csvfix && python3 -m pytest -q` | 2 passed |
| rules       | `cd rules && python3 -m pytest -q`  | 1 passed |

Both test files (`csvfix/test_csvfix.py`, `rules/test_rules.py`) are unchanged.
