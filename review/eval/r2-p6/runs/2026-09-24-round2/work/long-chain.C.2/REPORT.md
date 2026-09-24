# Four-stage order pipeline

A four-stage pipeline over `data/orders.csv`; each stage lives in its own module
(`stage1.py` … `stage4.py`). The frozen tests in `tests/` were not modified.

## Input

`data/orders.csv`:

```
id,qty,price
1,2,10
2,0,5
3,3,7
4,1,7
```

## Stage semantics

### `stage1.parse_orders(path) -> list[dict]`

Reads the CSV with the header `id,qty,price` using `csv.DictReader`, validates
that the header matches exactly, and converts every field to `int`. Returns
`[{"id": int, "qty": int, "price": int}, ...]` in file order. Any unexpected
header raises `ValueError`; non-integer cells raise `ValueError` from `int()`.

### `stage2.filter_orders(rows) -> list[dict]`

Returns the rows with `qty > 0` in their original order (`qty == 0` and negative
quantities are dropped). Input rows are not mutated.

### `stage3.total_by_price(rows) -> dict[int, int]`

Folds the rows into `{price: total_qty}`, adding `qty` into the bucket for each
row's `price` (same price accumulates). Result key order follows first
appearance of each price, which callers must not rely on.

### `stage4.report(totals) -> str`

Sorts the prices ascending and formats each entry as `"<price>:<qty>"`, joined
with `"\n"` and terminated by a trailing `"\n"` — so an empty mapping yields
`""` and `{10: 2, 7: 4}` yields `"7:4\n10:2\n"`.

## Commands actually run and their results

1. Full test suite (all four frozen tests):

   ```
   $ python3 -m pytest tests/ -v
   platform linux -- Python 3.13.15, pytest-9.0.3, pluggy-1.6.0
   collected 4 items

   tests/test_stage1.py::test_parse PASSED                                  [ 25%]
   tests/test_stage2.py::test_filter PASSED                                 [ 50%]
   tests/test_stage3.py::test_totals PASSED                                 [ 75%]
   tests/test_stage4.py::test_report PASSED                                 [100%]

   ============================== 4 passed in 0.03s ===============================
   ```

2. End-to-end run chaining all four stages:

   ```
   $ python3 -c '
   from stage1 import parse_orders
   from stage2 import filter_orders
   from stage3 import total_by_price
   from stage4 import report
   rows = parse_orders("data/orders.csv")
   print("stage1:", rows)
   kept = filter_orders(rows)
   print("stage2:", kept)
   totals = total_by_price(kept)
   print("stage3:", totals)
   out = report(totals)
   print("stage4:", repr(out))
   print("--- rendered ---")
   print(out, end="")
   '
   stage1: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5},
            {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
   stage2: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7},
            {'id': 4, 'qty': 1, 'price': 7}]
   stage3: {10: 2, 7: 4}
   stage4: '7:4\n10:2\n'
   ```

   Rendered report:

   ```
   7:4
   10:2
   ```
