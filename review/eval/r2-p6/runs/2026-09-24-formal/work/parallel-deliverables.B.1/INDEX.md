# Deliverables

Two independent, self-contained deliverables in this workspace.

## 1. `csvfix/` — CSV line parsing and amount totals

`csvfix/impl.py` implements the semantics described by `csvfix/test_csvfix.py`:

- `parse(line)` — splits one line into fields, trims whitespace from every field,
  and returns `None` unless the line has exactly 3 columns. Double quotes are
  honoured so a comma inside a quoted field is not a separator (`""` is an
  escaped quote).
- `total(rows)` — sums the **third** column (the amount) of the rows, skipping
  rows with no third column and rows whose amount is blank/whitespace; a
  non-numeric amount raises `ValueError` via `int`.

Verification:

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s
```

Additional manual probes (all as expected): `parse(' a , 2 , x ') == ['a','2','x']`,
`parse('a,2') is None`, `parse('"a,b", 2 , x ') == ['a,b','2','x']`,
`total([["a","1","2"],["b","2",""],["c","3"],["d","4","   "]]) == 2`,
`total([["a","1","oops"]])` raises `ValueError`.

## 2. `rules/` — rule evaluator

`rules/engine.py` implements `evaluate(rules, facts)` for the two rule forms in
`rules/test_rules.py`:

- `{"all": [...]}` → `True` only if every listed fact is truthy.
- `{"any": [...]}` → `True` if at least one listed fact is truthy.
- Missing facts count as falsy; an unrecognised rule returns `False`.
- Always returns a real `bool` (`is True` / `is False` assertions hold).

Verification:

```
$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```

Additional manual probes: `evaluate({"all": []}, {}) is True`,
`evaluate({"any": []}, {}) is False`,
`evaluate({"all": ["missing"]}, {"a": True}) is False`.

## Tests untouched

`csvfix/test_csvfix.py` and `rules/test_rules.py` were **not** modified. Checksums:

```
588db2b78e9d1365dd8d7563abe70812bf958955665a95ad02b1e6a16c793724  csvfix/test_csvfix.py
4c425638c58c34872b0800f5fe19092c011785e123c15a1c69e2e99d762a5af8  rules/test_rules.py
```

## Files changed

- `csvfix/impl.py` (implementation fixed)
- `rules/engine.py` (implementation added)
- `INDEX.md` (this file)
