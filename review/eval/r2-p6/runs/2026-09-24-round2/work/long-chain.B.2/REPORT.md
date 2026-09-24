# REPORT

四阶段流水线实现，输入为 `data/orders.csv`（表头 `id,qty,price`）。

## 各阶段语义

### stage1.parse_orders(path)
用标准库 `csv.DictReader` 读取 CSV，把每行的 `id`、`qty`、`price` 转成 `int`，
返回 `[{"id": int, "qty": int, "price": int}, ...]`，不改变行的顺序。

### stage2.filter_orders(rows)
返回 `qty > 0` 的行，保持输入顺序；`qty <= 0`（含 0 与负数）被丢弃。

### stage3.total_by_price(rows)
按 `price` 聚合 `qty`，返回 `{price: 总 qty}`；同一价格的 qty 累加。

### stage4.report(totals)
按价格升序遍历，生成多行字符串 `"<price>:<qty>"`，每行以 `\n` 结尾
（因此整体以换行结束）；价格升序由 `sorted(totals)` 保证。

## 真实运行过的命令与结果

1. 运行冻结测试：

   ```
   $ python -m pytest tests/ -v
   ```

   结果：

   ```
   collected 4 items

   tests/test_stage1.py::test_parse PASSED                                  [ 25%]
   tests/test_stage2.py::test_filter PASSED                                 [ 50%]
   tests/test_stage3.py::test_totals PASSED                                 [ 75%]
   tests/test_stage4.py::test_report PASSED                                 [100%]

   ============================== 4 passed in 0.04s ===============================
   ```

2. 端到端串联四阶段：

   ```
   $ python -c "from stage1 import parse_orders; from stage2 import filter_orders; \
   from stage3 import total_by_price; from stage4 import report; \
   rows=parse_orders('data/orders.csv'); print('parsed:',rows); \
   kept=filter_orders(rows); print('kept:',kept); \
   t=total_by_price(kept); print('totals:',t); \
   print('report:'); print(report(t),end='')"
   ```

   结果：

   ```
   parsed: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
   kept: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
   totals: {10: 2, 7: 4}
   report:
   7:4
   10:2
   ```

## 文件清单
- `stage1.py`
- `stage2.py`
- `stage3.py`
- `stage4.py`
- `REPORT.md`（本文件）

测试文件 `tests/` 未做任何修改。
