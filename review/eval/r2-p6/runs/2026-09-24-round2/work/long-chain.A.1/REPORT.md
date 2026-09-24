# Four-Stage Order Pipeline — Report

A four-stage pipeline, one file per stage. Each stage is a pure function that
takes the previous stage's output.

## Files

| File | Function | Semantics |
| --- | --- | --- |
| `stage1.py` | `parse_orders(path)` | Reads a CSV whose header is `id,qty,price`, using `csv.DictReader`. Returns a list of `{"id": int, "qty": int, "price": int}` dicts, in file order. |
| `stage2.py` | `filter_orders(rows)` | Drops every row with `qty <= 0`, preserving the original order. |
| `stage3.py` | `total_by_price(rows)` | Returns `{price: total_qty}`, summing `qty` across rows that share a price. |
| `stage4.py` | `report(totals)` | Renders keys of `totals` in ascending price order as `"<price>:<qty>"` lines joined by `\n`, with a trailing newline. Empty input yields `""`. |

`data/orders.csv`:

```
id,qty,price
1,2,10
2,0,5
3,3,7
4,1,7
```

## Commands actually run

All commands were run from the workspace root so that the tests' relative
`data/orders.csv` path resolves.

### 1. Frozen test suite

```
$ python3 -m pytest tests/ -v
```

Result:

```
============================= test session starts ==============================
platform linux -- Python 3.13.15, pytest-9.0.3, pluggy-1.6.0
collecting ... collected 4 items

tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]

============================== 4 passed in 0.04s ===============================
```

### 2. End-to-end pipeline

```
$ python3 -c "
from stage1 import parse_orders
from stage2 import filter_orders
from stage3 import total_by_price
from stage4 import report

rows = parse_orders('data/orders.csv')
kept = filter_orders(rows)
totals = total_by_price(kept)
print(repr(rows))
print(repr(kept))
print(repr(totals))
print(repr(report(totals)))
print(report(totals), end='')
"
```

Result:

```
[{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
[{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
{10: 2, 7: 4}
'7:4\n10:2\n'
7:4
10:2
```

## Outcome

- Stage 1 parses all 4 rows; the `qty=0` row (id 2, price 5) is present here.
- Stage 2 removes that row, leaving ids 1, 3, 4 in order.
- Stage 3 aggregates price 7 to `qty 3 + qty 1 = 4` and price 10 to `qty 2`.
- Stage 4 emits `7:4` then `10:2`, newline-terminated.

All four frozen tests pass; test files were not modified.
