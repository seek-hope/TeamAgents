# REPORT

## Overview

A four-stage pipeline implemented as one file per stage. Stage *n* consumes
the output of stage *n-1*; data flows as follows:

```
data/orders.csv --stage1--> list[dict] --stage2--> list[dict] --stage3--> dict --stage4--> str
```

## Stage semantics

### `stage1.parse_orders(path)`

Reads the CSV at `path` (header `id,qty,price`) with `csv.DictReader` and
returns a list of dicts in file order. Every field is cast to `int`, so the
result is `[{"id": int, "qty": int, "price": int}, ...]`.

### `stage2.filter_orders(rows)`

Returns a new list containing only rows with `qty > 0` (`row["qty"] > 0`),
preserving the original ordering. Input rows are not mutated.

### `stage3.total_by_price(rows)`

Folds the rows into a single mapping `{price: total_qty}`: for each row the
row's `qty` is added to the running total keyed by its `price`, so rows with
the same price accumulate.

### `stage4.report(totals)`

Renders the mapping as one `"<price>:<qty>"` line per entry, ordered by
ascending price (`sorted(totals)`), and joins the lines with `\n`, including a
trailing newline.

## Commands actually run

All commands were executed from the repository root
(`.../work/long-chain.A.2`).

### 1. Run the frozen tests

```
$ python -m pytest tests/ -v
```

Result:

```
============================= test session starts ==============================
platform linux -- Python 3.13.15, pytest-9.0.3, pluggy-1.6.0 -- /opt/miniconda3/bin/python
cachedir: .pytest_cache
rootdir: .../work/long-chain.A.2
plugins: anyio-4.12.1
collecting ... collected 4 items

tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]

============================== 4 passed in 0.04s ===============================
```

### 2. Run the pipeline end to end

```
$ python -c "
from stage1 import parse_orders
from stage2 import filter_orders
from stage3 import total_by_price
from stage4 import report
rows = parse_orders('data/orders.csv')
print('parsed:', rows)
kept = filter_orders(rows)
print('kept:', kept)
totals = total_by_price(kept)
print('totals:', totals)
print('report:')
print(report(totals), end='')
"
```

Result:

```
parsed: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
kept: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
totals: {10: 2, 7: 4}
report:
7:4
10:2
```

## Notes

- `tests/` was not modified; only `stage1.py`–`stage4.py` and this `REPORT.md`
  were written.
- `data/orders.csv` contains four rows; row `id=2` has `qty=0` and is dropped
  by stage 2. Prices 7 (rows 3 and 4) and 10 (row 1) survive, giving totals
  `{10: 2, 7: 4}` and the report `"7:4\n10:2\n"`.
