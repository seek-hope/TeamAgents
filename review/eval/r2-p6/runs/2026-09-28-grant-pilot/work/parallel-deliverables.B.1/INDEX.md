# INDEX

Two independent deliverables live here. Each has its own implementation file and
its own (unmodified) test suite.

## 1. `csvfix/` — CSV field parsing and summing

* **What it is:** `csvfix/impl.py` provides two functions used on 3-column CSV data:
  * `parse(line)` — splits a line on `,`, strips surrounding whitespace from every
    field, and returns the resulting list of 3 strings; returns `None` when the
    line does not have exactly 3 columns.
  * `total(rows)` — sums the amount column (third field), skipping rows whose
    amount field is blank; amounts are parsed as integers.
* **Contract source:** `csvfix/test_csvfix.py` (the specification; not changed).

### Verification

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.02s
```

## 2. `rules/` — tiny rule engine

* **What it is:** `rules/engine.py` implements `evaluate(rules, facts)` over a
  `{name: bool}` fact mapping:
  * `{"all": [k, ...]}` → `True` only when every listed fact is truthy
    (empty list is vacuously `True`).
  * `{"any": [k, ...]}` → `True` when at least one listed fact is truthy
    (empty list is `False`).
  * Unknown/missing facts count as `False`; an unrecognised rule shape returns
    `False`.
* **Contract source:** `rules/test_rules.py` (the specification; not changed).

### Verification

```
$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

## Files touched

* `csvfix/impl.py` — replaced the buggy `parse` (no trimming / no column check)
  and `total` (blank handling) with the specified behaviour.
* `rules/engine.py` — replaced the `return False` stub with an `all`/`any`
  implementation.
* `INDEX.md` — this file.

Test files (`csvfix/test_csvfix.py`, `rules/test_rules.py`) were **not** modified.
