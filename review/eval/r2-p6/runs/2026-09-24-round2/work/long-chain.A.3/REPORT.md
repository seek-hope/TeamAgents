# Four-Stage Order Pipeline — Report

A four-stage pipeline, one stage per file. Each stage is a pure function that
consumes the previous stage's output; the stages are independent modules with
no cross-imports.

## Files

| File | Function | Role |
| --- | --- | --- |
| `stage1.py` | `parse_orders(path)` | CSV -> typed rows |
| `stage2.py` | `filter_orders(rows)` | drop `qty <= 0` |
| `stage3.py` | `total_by_price(rows)` | aggregate qty per price |
| `stage4.py` | `report(totals)` | render sorted report string |

`data/orders.csv` and the four files under `tests/` were **not** modified.

## Stage semantics

1. **`stage1.parse_orders(path)`** — Opens the CSV at `path` with the header
   `id,qty,price`. The header row is skipped (matched case-insensitively by
   field name), fully blank lines are ignored, and each remaining record is
   returned as `{"id": int, "qty": int, "price": int}`. File order is
   preserved. Output is a `list`.
2. **`stage2.filter_orders(rows)`** — Returns a new `list` containing only
   rows with `qty > 0` (so `qty <= 0`, including negatives and zeros, is
   dropped). Original order is preserved and the input list is not mutated.
3. **`stage3.total_by_price(rows)`** — Sums `qty` per distinct `price` and
   returns `{price: total_qty}`. Empty input returns `{}`.
4. **`stage4.report(totals)`** — Returns one line per price as
   `"<price>:<qty>"`, sorted by price ascending, terminated by a trailing
   newline. Empty input returns `""`.

The stages compose as:
`report(total_by_price(filter_orders(parse_orders("data/orders.csv"))))`.

## Commands actually run and results

All commands were run from the workspace root
(`.../long-chain.A.3`) with Python 3.13.15 / pytest 9.0.3.

### 1. Frozen test suite

```
$ python -m pytest tests/ -v
```

Result (verbatim summary):

```
tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]
============================== 4 passed in 0.04s ===============================
```

### 2. End-to-end pipeline on the shipped data

```
$ python -c "
from stage1 import parse_orders
from stage2 import filter_orders
from stage3 import total_by_price
from stage4 import report
rows = parse_orders('data/orders.csv')
print('stage1:', rows)
kept = filter_orders(rows)
print('stage2:', kept)
totals = total_by_price(kept)
print('stage3:', totals)
print('stage4:', repr(report(totals)))
"
```

Actual output:

```
stage1: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
stage2: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
stage3: {10: 2, 7: 4}
stage4: '7:4\n10:2\n'
```

So `data/orders.csv` yields the report:

```
7:4
10:2
```

### 3. Edge cases (negative qty, blank line, empty report)

```
$ python -c "
import tempfile, os
from stage1 import parse_orders
from stage2 import filter_orders
from stage3 import total_by_price
from stage4 import report
p = tempfile.mktemp(suffix='.csv')
open(p,'w').write('id,qty,price\n9,-3,4\n10,5,2\n\n11,0,4\n')
rows = parse_orders(p); print('rows:', rows)
print('filter:', filter_orders(rows))
print('totals:', total_by_price(filter_orders(rows)))
print('report:', repr(report(total_by_price(filter_orders(rows)))))
print('empty report:', repr(report({})))
os.remove(p)
"
```

Actual output:

```
rows: [{'id': 9, 'qty': -3, 'price': 4}, {'id': 10, 'qty': 5, 'price': 2}, {'id': 11, 'qty': 0, 'price': 4}]
filter: [{'id': 10, 'qty': 5, 'price': 2}]
totals: {2: 5}
report: '2:5\n'
empty report: ''
```

This confirms negative quantities are dropped, blank lines are ignored, and
the empty mapping renders as the empty string.

## Notes on unchanged inputs

`md5sum tests/*.py` was run after implementation; the test files were only
read, never written, so their contents are as shipped.
