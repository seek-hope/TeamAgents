# 四阶段订单处理流水线 — REPORT

本仓库实现一条四阶段流水线，每阶段一个文件。测试文件位于 `tests/`，**未做任何修改**（SHA-256 见文末）。

## 各阶段语义

| 阶段 | 文件 | 函数 | 语义 |
| --- | --- | --- | --- |
| 1 | `stage1.py` | `parse_orders(path)` | 用 `csv.DictReader` 读取表头为 `id,qty,price` 的 CSV，逐行转换为 `{"id": int, "qty": int, "price": int}`，保持文件中的原始顺序。 |
| 2 | `stage2.py` | `filter_orders(rows)` | 丢弃 `qty <= 0` 的行（即只保留 `qty > 0`），保持原顺序，返回新列表。 |
| 3 | `stage3.py` | `total_by_price(rows)` | 按 `price` 累加 `qty`，返回 `{price: 总 qty}` 字典。 |
| 4 | `stage4.py` | `report(totals)` | 按 `price` 升序输出多行字符串，每行格式 `"<price>:<qty>"`，并保证以换行符结尾。 |

正数价格排序按升序；字典键为 `int`，格式化为十进制无前导零。

## 真实运行过的命令与结果

### 1. 运行冻结测试

```
$ python3 -m pytest tests/ -v
```

输出（节选）：

```
platform linux -- Python 3.13.15, pytest-9.0.3, pluggy-1.6.0
collected 4 items

tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]

============================== 4 passed in 0.04s ===============================
```

结果：**4 passed**。

### 2. 端到端串联四阶段

```
$ python3 -c "
from stage1 import parse_orders
from stage2 import filter_orders
from stage3 import total_by_price
from stage4 import report
rows = parse_orders('data/orders.csv')
print('parsed   :', rows)
kept = filter_orders(rows)
print('filtered :', kept)
totals = total_by_price(kept)
print('totals   :', totals)
print('report   :')
print(report(totals), end='')
"
```

输出：

```
parsed   : [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
filtered : [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
totals   : {10: 2, 7: 4}
report   :
7:4
10:2
```

`data/orders.csv` 中 `id=2` 的 `qty=0` 被阶段 2 丢弃；价格 7 的 3 与 1 累加为 4；价格升序输出为 `7:4` 与 `10:2`，末尾带换行。

## 冻结测试文件校验和（运行前记录，未修改）

```
13aa797e9aef3c9aba1c20677f6fec223964e6c285658c224dae79e3efdc3670  tests/test_stage1.py
b48a335be723c00645c2305619266096635a4c2de66703e2987af54a4ece56e5  tests/test_stage2.py
ed5c68b1370162f168d99306dc6cd7feca0d8668757ed0acb7d97a57b130ad32  tests/test_stage3.py
c3d2971eca0f4e523fd9a27a4d017ccc7797dbaf8b8abeffdd411c300fe8c453  tests/test_stage4.py
```
