# REPORT

四阶段 CSV 订单处理流水线。每个阶段一个文件，`stageN` 的输入为上一阶段的输出。

## 各阶段语义

### stage1.parse_orders(path)

读取 CSV 文件（表头 `id,qty,price`），用 `csv.DictReader` 逐行解析，把每行的三个字段
转成 `int`，返回字典列表：`[{"id": int, "qty": int, "price": int}, ...]`。
保持文件中的原始行顺序。

### stage2.filter_orders(rows)

返回新列表，仅保留 `qty > 0` 的行（丢弃 `qty <= 0`），保持原顺序。

### stage3.total_by_price(rows)

按 `price` 聚合 `qty`，返回 `{price: 总 qty}`；相同价格累加。

### stage4.report(totals)

按价格升序生成多行字符串，每行 `"<price>:<qty>"`，末尾带换行符。

## 真实运行过的命令与结果

### 1. 冻结测试

```
$ python -m pytest tests/ -v
```

结果：4 个测试全部通过。

```
tests/test_stage1.py::test_parse PASSED                                  [ 25%]
tests/test_stage2.py::test_filter PASSED                                 [ 50%]
tests/test_stage3.py::test_totals PASSED                                 [ 75%]
tests/test_stage4.py::test_report PASSED                                 [100%]

============================== 4 passed in 0.04s ===============================
EXIT=0
```

### 2. 端到端流水线

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

结果：

```
stage1: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 2, 'qty': 0, 'price': 5}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
stage2: [{'id': 1, 'qty': 2, 'price': 10}, {'id': 3, 'qty': 3, 'price': 7}, {'id': 4, 'qty': 1, 'price': 7}]
stage3: {10: 2, 7: 4}
stage4: '7:4\n10:2\n'
```

其中输入 `data/orders.csv` 内容为：

```
id,qty,price
1,2,10
2,0,5
3,3,7
4,1,7
```

说明：`qty=0` 的第 2 行被 stage2 丢弃，因此价格 5 不出现在最终报告中；
价格 7 的两行（qty 3 与 1）合并为 4。
