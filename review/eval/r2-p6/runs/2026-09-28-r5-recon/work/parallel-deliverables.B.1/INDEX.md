# Deliverables Index

Two independent deliverables live in this workspace; each has its own test suite and
was verified by running that suite.

## 1. `csvfix/` — CSV parsing/aggregation fix

- `csvfix/impl.py`: implemented `parse` and `total`.
  - `parse(line)` trims whitespace from every field and returns `None` unless the line
    has exactly 3 columns.
  - `total(rows)` treats the **third** column as the amount and skips blank amount
    fields instead of counting them as 0.
- `csvfix/test_csvfix.py` was **not modified**.

Verification:

```
$ cd csvfix && python3 -m pytest -q
2 passed in 0.02s
```

## 2. `rules/` — rule engine

- `rules/engine.py`: implemented `evaluate(rules, facts)` for the two rule shapes
  `{"all": [...]}` (all named facts truthy) and `{"any": [...]}` (at least one truthy).
  An empty `all` is vacuously `True`; an empty `any` is `False`; unknown operators raise
  `ValueError`.
- `rules/test_rules.py` was **not modified**.

Verification:

```
$ cd rules && python3 -m pytest -q
1 passed in 0.01s
```

## How I verified

Both commands were run from this workspace root immediately after the edits; both
exited 0 with the output shown above ("2 passed" for csvfix, "1 passed" for rules).
