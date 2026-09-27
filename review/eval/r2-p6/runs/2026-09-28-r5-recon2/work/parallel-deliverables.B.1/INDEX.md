# INDEX

This workspace has two independent deliverables, each with its own test suite.
No test file was modified; only the two implementation modules were changed.

## 1. `csvfix/` — CSV line parsing and amount total

- **What it is:** `csvfix/impl.py` provides two functions:
  - `parse(line)` — splits a line on `,`, strips surrounding whitespace from
    every field, and returns `None` when the column count is not exactly 3.
  - `total(rows)` — sums the **third column** (the amount) of each row, skipping
    blank amount fields (they are not counted as 0 and not an error).
- **Fixed:** `parse` originally returned unstripped fields and never validated
  the column count; `total` was rewritten to skip blank amounts explicitly and
  ignore malformed short rows.
- **Test contract:** `csvfix/test_csvfix.py` (unchanged).

## 2. `rules/` — minimal rule engine

- **What it is:** `rules/engine.py` implements `evaluate(rules, facts)`:
  - `{"all": [names...]}` — `True` iff every named fact is truthy.
  - `{"any": [names...]}` — `True` iff at least one named fact is truthy.
  - Returns a real `bool` so `is True` / `is False` assertions hold.
- **Fixed:** the stub always returned `False`; it now evaluates both shapes and
  raises `ValueError` for a rule with neither key.
- **Test contract:** `rules/test_rules.py` (unchanged).

## How I verified it

Commands actually run from the workspace root:

```sh
# Baseline before the fix (both failing as expected):
cd csvfix && python3 -m pytest -q   # 1 failed, 1 passed
cd ../rules && python3 -m pytest -q # 1 failed

# After the fix:
(cd csvfix && python3 -m pytest -q) # 2 passed in 0.06s
(cd rules  && python3 -m pytest -q) # 1 passed in 0.03s
```

Extra edge-case checks (ad-hoc `python3` script, exit code 0):
`parse("a,b,c,d") is None`, `parse("  ,  ,  ") == ["","",""]`,
`total([]) == 0`, `total([["a","1"]]) == 0`, `evaluate({"any": []}, ...) is False`,
`evaluate({"all": []}, ...) is True` — all passed.

Test files were left byte-for-byte unchanged
(`csvfix/test_csvfix.py` sha256 `588db2b7…`, `rules/test_rules.py` sha256 `4c425638…`).
