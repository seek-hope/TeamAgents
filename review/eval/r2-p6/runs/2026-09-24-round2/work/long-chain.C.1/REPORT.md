# REPORT.md

Four-stage order pipeline. Each stage lives in its own module and is a pure
function of the previous stage's output; `tests/` was not modified.

## Stage semantics

- **`stage1.parse_orders(path)`** (`stage1.py`) — Reads the CSV at `path`
  (header `id,qty,price`) with `csv.DictReader` and returns a list of
  `{"id": int, "qty": int, "price": int}` dicts, one per data row, in file
  order. All three fields are cast to `int`.
- **`stage2.filter_orders(rows)`** (`stage2.py`) — Returns a new list containing
  only rows with `qty > 0` (i.e. discards `qty <= 0`), preserving the original
  order.
- **`stage3.total_by_price(rows)`** (`stage3.py`) — Aggregates `qty` per
  distinct `price` and returns `{price: total_qty}`; equal prices are summed.
  Output key order is insertion order.
- **`stage4.report(totals)`** (`stage4.py`) — Renders one line per price in
  ascending price order as `"<price>:<qty>"` joined by `\n`, with a trailing
  newline, and returns the resulting string.

## Commands actually run

All commands were run from the workspace root
(`.../work/long-chain.C.1`) on 2026-09-24.

1. Frozen test suite:

   ```console
   $ python -m pytest tests/ -q
   ....                                                                     [100%]
   4 passed in 0.01s
   ```

2. End-to-end pipeline over the real data file:

   ```console
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
   out = report(totals)
   print('stage4 repr:', repr(out))
   "
   stage1: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
   stage2: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
   stage3: {10: 2, 7: 4}
   stage4 repr: '7:4\n10:2\n'
   ```

## Result

- Input `data/orders.csv`: `(id=1,qty=2,price=10)`, `(id=2,qty=0,price=5)`,
  `(id=3,qty=3,price=7)`, `(id=4,qty=1,price=7)`.
- Stage 2 drops the `qty=0` row (id 2).
- Stage 3 sums the two `price=7` rows into `4`, keeping `price=10 -> 2`.
- Stage 4 emits:

  ```text
  7:4
  10:2
  ```

All four frozen tests (`tests/test_stage1.py` … `tests/test_stage4.py`) pass.
