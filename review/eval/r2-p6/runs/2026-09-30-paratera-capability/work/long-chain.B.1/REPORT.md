# REPORT

四阶段流水线实现，用于读订单 CSV 并汇总每个价格的 qty。测试文件未修改。

## 阶段语义

### stage1.parse_orders(path)
用 `csv.DictReader` 读取表头为 `id,qty,price` 的 CSV，逐行把三个字段转成 `int`，
返回 `[{"id": int, "qty": int, "price": int}, ...]`，保持文件中的原始行序。

### stage2.filter_orders(rows)
列表推导过滤掉 `qty <= 0` 的行，保持其余行的相对顺序；不修改输入。

### stage3.total_by_price(rows)
用 dict 累加同一 `price` 的 `qty`，返回 `{price: 总 qty}`。
累加只依赖输入行的顺序，对同一价格的多行求和。

### stage4.report(totals)
按 `price` 升序，把每个键值对格式化为一行 `"<price>:<qty>"`，拼接为多行字符串，
并以换行结尾（每个键一行，所以最后一行后仍有 `\n`）。

## 真实运行过的命令与结果

### 1. 端到端串联四阶段

```
$ python3 -c "
import stage1, stage2, stage3, stage4
rows = stage1.parse_orders('data/orders.csv')
f = stage2.filter_orders(rows)
t = stage3.total_by_price(f)
r = stage4.report(t)
print(repr(rows)); print(repr(f)); print(repr(t)); print(repr(r))
"
```

输出：

```
[{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
[{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
{10: 2, 7: 4}
'7:4\n10:2\n'
```

### 2. 运行冻结测试

```
$ python3 -m pytest tests/ -v
```

输出（摘要）：

```
collected 4 items

tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]

============================== 4 passed in 0.03s ===============================
```

## 交付文件

- `stage1.py` — `parse_orders`
- `stage2.py` — `filter_orders`
- `stage3.py` — `total_by_price`
- `stage4.py` — `report`
- `REPORT.md` — 本文件

测试文件（`tests/test_stage1.py` … `tests/test_stage4.py`）与 `data/orders.csv` 均未修改。
