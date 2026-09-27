# INDEX

Two independent deliverables live in this workspace.

## 1. `csvfix/` — CSV parsing/totalling fix

`csvfix/impl.py` previously split naively and summed the wrong column.
It now matches the semantics asserted by `csvfix/test_csvfix.py`:

- `parse(line)`: splits on commas, strips whitespace from every field, and
  returns the field list only when there are exactly 3 columns; otherwise
  returns `None`.
- `total(rows)`: treats the third column (index 2) as the amount, skips rows
  whose amount field is blank/whitespace-only, and coerces the amount to `int`
  before summing.

Test file `csvfix/test_csvfix.py` was **not** modified.

## 2. `rules/` — rule evaluation engine

`rules/engine.py` implements `evaluate(rules, facts)` to match
`rules/test_rules.py`:

- `{"all": [names...]}` → `True` iff every named fact in `facts` is truthy
  (empty list → `True`, vacuous truth); missing names are falsy.
- `{"any": [names...]}` → `True` iff at least one named fact is truthy
  (empty list → `False`); missing names are falsy.
- Returns real Python `bool` values so the tests' `is True` / `is False` hold.

Test file `rules/test_rules.py` was **not** modified.

## How it was verified (real commands and results)

Run from this workspace, after the fixes landed:

```text
$ (cd csvfix && python3 -m pytest -q)
..                                                                       [100%]
2 passed in 0.01s
EXIT=0
```

```text
$ (cd rules && python3 -m pytest -q)
.                                                                        [100%]
1 passed in 0.02s
EXIT=0
```

Test-file integrity was checked too (they still match the originals):
`md5sum csvfix/test_csvfix.py rules/test_rules.py` →
`8d5ddb899fec22f5616a12c186fef11e` and `12b10bdc4b1b2188c37fe54ebdfd64f0`.
