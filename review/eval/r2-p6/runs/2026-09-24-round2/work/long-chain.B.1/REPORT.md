# REPORT.md — Four-stage order pipeline

## Files

| File | Contents |
| --- | --- |
| `stage1.py` | `parse_orders(path)` |
| `stage2.py` | `filter_orders(rows)` |
| `stage3.py` | `total_by_price(rows)` |
| `stage4.py` | `report(totals)` |

The frozen tests under `tests/` were not modified.

## Stage semantics

### Stage 1 — `stage1.parse_orders(path)`

Reads a CSV file whose header is `id,qty,price` and returns a list of dicts
`[{"id": int, "qty": int, "price": int}, ...]`. Implementation uses
`csv.DictReader` and casts every field with `int(...)`. File order is preserved.

Input `data/orders.csv`:

```
id,qty,price
1,2,10
2,0,5
3,3,7
4,1,7
```

Result: `[{"id":1,"qty":2,"price":10},{"id":2,"qty":0,"price":5},{"id":3,"qty":3,"price":7},{"id":4,"qty":1,"price":7}]`.

### Stage 2 — `stage2.filter_orders(rows)`

Returns a new list containing only rows with `qty > 0` (`row["qty"] > 0`).
Rows with `qty <= 0` (including `qty == 0`) are dropped. Relative order of the
surviving rows is unchanged.

On the stage-1 output this drops id 2 (`qty=0`), leaving `[{"id":1,"qty":2,"price":10},{"id":3,"qty":3,"price":7},{"id":4,"qty":1,"price":7}]`.

### Stage 3 — `stage3.total_by_price(rows)`

Aggregates quantities by price into `{price: total_qty}`; rows sharing a price
are summed. Dict insertion order follows first appearance, which is irrelevant
because stage 4 sorts.

On the stage-2 output: `{10: 2, 7: 3 + 1}` → `{10: 2, 7: 4}`.

### Stage 4 — `stage4.report(totals)`

Emits one line `"<price>:<qty>"` per price in **ascending price order**, joined
with `"\n"` and terminated by a final newline. An empty `totals` yields `""`
(no lines, hence no trailing newline).

On the stage-3 output: `"7:4\n10:2\n"`.

## End-to-end run: commands and actual results

### 1. Test suite

```
$ python3 -m pytest tests/ -v
============================= test session starts ==============================
platform linux -- Python 3.13.15, pytest-9.0.3, pluggy-1.6.0 -- /opt/miniconda3/bin/python3
cachedir: .pytest_cache
rootdir: /home/rimuru/Projects/Code/for_fun/TeamAgents/review/eval/r2-p6/runs/2026-09-24-round2/work/long-chain.B.1
plugins: anyio-4.12.1
collecting ... collected 4 items

tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]

============================== 4 passed in 0.03s ===============================
```

(Working directory is the project root, which is required because
`tests/test_stage1.py` opens the relative path `data/orders.csv`.)

### 2. Full pipeline over the real data

```
$ python3 -c "
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
print('stage4 repr:', repr(report(totals)))
print('stage4 rendered:')
print(report(totals), end='')
"
stage1: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
stage2: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
stage3: {10: 2, 7: 4}
stage4 repr: '7:4\n10:2\n'
stage4 rendered:
7:4
10:2
```

## Outcome

All four frozen tests pass, and the composed pipeline on `data/orders.csv`
produces `"7:4\n10:2\n"`.
