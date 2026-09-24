# Deliverables Index

Two independent deliverables in this workspace, each fixed without touching its test file.

## 1. `csvfix/` — CSV field parsing and amount totaling

`impl.py` had two bugs. `parse(line)` now splits on commas, strips whitespace from every
field, and returns `None` when the column count is not exactly 3 (previously it returned the
raw `split` result). `total(rows)` sums the third column (the amount) and skips rows whose
amount field is blank. `test_csvfix.py` was left unmodified.

## 2. `rules/` — rule evaluation engine

`engine.evaluate(rules, facts)` was a stub returning `False`. It now supports a single-key
rule dict: `{"all": [...]}` is true when every named fact is truthy, and `{"any": [...]}` is
true when at least one is. Unknown rule shapes return `False`. `test_rules.py` was left
unmodified.

## Verification (real commands and results)

```
cd csvfix && python3 -m pytest -q
# ..  [100%]
# 2 passed in 0.00s   (exit 0)

cd rules && python3 -m pytest -q
# .  [100%]
# 1 passed in 0.00s   (exit 0)
```

Test files `csvfix/test_csvfix.py` and `rules/test_rules.py` were not modified.
