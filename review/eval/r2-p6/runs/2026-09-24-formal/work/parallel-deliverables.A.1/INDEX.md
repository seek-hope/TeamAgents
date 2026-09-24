# Deliverables

Two independent deliverables live in this workspace.

## 1. `csvfix/` — CSV parsing/total fix
`impl.py` provides `parse(line)` and `total(rows)`.
- `parse` trims every field and returns `None` unless the line has exactly 3 columns.
- `total` sums the amount (third) column and skips blank amount fields.

Test file `test_csvfix.py` was left untouched.

Verification:
```
$ cd csvfix && python3 -m pytest -q
2 passed in 0.00s   (exit 0)
```

## 2. `rules/` — rule engine
`engine.py` implements `evaluate(rules, facts)`:
- `{"all": [...]}` is true when every referenced fact is truthy.
- `{"any": [...]}` is true when at least one referenced fact is truthy.
- Unknown fact names count as `False`; unrecognized rule shapes return `False`.

Test file `test_rules.py` was left untouched.

Verification:
```
$ cd rules && python3 -m pytest -q
1 passed in 0.00s   (exit 0)
```

## Files changed
- `csvfix/impl.py` (implementation fixed)
- `rules/engine.py` (implementation added)
- `INDEX.md` (this file)
