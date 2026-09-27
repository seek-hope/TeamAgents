# Deliverables Index

Two independent deliverables live side by side in this workspace.

## 1. `csvfix/` — CSV parsing/aggregation fix
`impl.py` had a broken `parse`/`total`. `parse(line)` now strips every field and
returns `None` when the column count is not exactly 3; `total(rows)` uses the
third column as the amount and skips rows whose amount field is blank.
`test_csvfix.py` is the spec and was left untouched.

## 2. `rules/` — rule evaluation engine
`engine.evaluate(rules, facts)` implements the `{"all": [...]}` and
`{"any": [...]}` rule forms (a fact key is truthy per `facts`; missing facts are
falsy), returning a real `bool`. `test_rules.py` is the spec and was left untouched.

## How it was verified (real commands + results)
Run from this workspace directory:

```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.01s          # exit 0

$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s          # exit 0
```

Tests were not weakened — both test files are byte-identical to their baseline
SHA-256:

```
588db2b78e9d1365dd8d7563abe70812bf958955665a95ad02b1e6a16c793724  csvfix/test_csvfix.py
4c425638c58c34872b0800f5fe19092c011785e123c15a1c69e2e99d762a5af8  rules/test_rules.py
```

Spot checks of the required semantics:

```
$ python3 -c "import impl; print(impl.parse('a,2'))"          # -> None
$ python3 -c "import impl; print(impl.total([['a','1','2'],['b','2','']]))"  # -> 2
```
