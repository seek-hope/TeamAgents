# 12 独立交付物 —— 实现报告

所有实现的验收命令均在各自目录下真实运行，全部通过。未修改任何测试或验收文件
（`test_*.py`、`check.py` 保持不变），只改写了实现文件。

## 运行汇总

| # | 目录 | 命令 | 结果 |
|---|------|------|------|
| 1 | units/csvfix | `python3 -m pytest -q` | 2 passed |
| 2 | units/rules | `python3 -m pytest -q` | 1 passed |
| 3 | units/ledger | `python3 -m pytest -q` | 2 passed |
| 4 | units/schedule | `python3 -m pytest -q` | 2 passed |
| 5 | units/intervals | `python3 -m pytest -q` | 7 passed |
| 6 | units/flags | `python3 -m pytest -q` | 8 passed |
| 7 | units/measure | `python3 -m pytest -q` | 1 passed |
| 8 | units/slugify | `python3 -m pytest -q` | 1 passed |
| 9 | units/ranges | `python3 -m pytest -q` | 1 passed |
| 10 | units/alpha | `python3 check.py` | `alpha ok` |
| 11 | units/beta | `python3 check.py` | `beta ok` |
| 12 | units/chain | `python3 -m pytest -q tests/` | 4 passed |

（exit code 均为 0。）

## 逐块语义边界

### 1. units/csvfix —— `impl.py`
- `parse(line)`：用 `csv.reader` 解析单行，**支持双引号包裹字段**；对每个字段
  `strip()` 去首尾空白；**要求恰好 3 列，列数不是 3 返回 `None`**。
  例：`' a , 2 , x ' -> ["a","2","x"]`；`'a,2' -> None`。
- `total(rows)`：取每行**第三列**（索引 2）当作金额求和；**空字段跳过**；
  列不足 3 的行跳过；非空但非整数的字段抛 `ValueError`。
  例：`[["a","1","2"],["b","2",""]] -> 2`。

### 2. units/rules —— `engine.evaluate(rules, facts)`
- `{"all": [...]}`：全部键在 `facts` 中为真才返回 `True`（空列表 → `True`）。
- `{"any": [...]}`：任意一键为真即 `True`（空列表 → `False`）。
- 缺失键视为假；显式返回 Python `True`/`False`（测试用 `is True/False`）。

### 3. units/ledger —— `Ledger.apply` / `balance`
- `apply(rows, op)`：逐行校验 `kind` 只能是 `"deposit"`/`"withdraw"`，
  **金额为负抛 `ValueError`**（校验针对所有行，不只目标账户）；
  返回属于账户 `op` 的行，保持原始顺序。
- `balance(rows, account)`：`deposit` 累加、`withdraw` 累减；无该账户返回 `0`。

### 4. units/schedule —— `slots` / `overlaps`
- `slots(ranges, minutes)`：闭区间按起点排序；相邻两段空隙
  `next_lo - prev_hi < minutes` 视为同段并合并（合并时取较大 `hi`）；
  空隙等于 `minutes` 不合并。空输入 `[]`。
  例：`slots([(0,60),(30,90),(120,150)], 30) == [(0,90),(120,150)]`。
- `overlaps(a, b)`：闭区间交叠判定 `a[0] < b[1] and b[0] < a[1]`，
  端点正好相接不算交叠。

### 5. units/intervals —— `Impl`
- `merge(ranges, gap=0)`：按 `lo` 升序；相邻两段**缺失整数个数**
  `next.lo - prev.hi - 1 <= gap` 才合并；嵌套/反向的区间被吸收；
  空输入 `[]`。`gap=0` 时相邻（缺失 0 个）合并，真正有空隙则不合并。
- `subtract(ranges, hole)`：先 `merge` 归一化输入，再用闭区间 `hole` 挖除：
  不相交段保留；被完全覆盖的段消失；横跨的段分裂为 `(lo, hlo-1)` 与 `(hhi+1, hi)`。
- `total_length(ranges)`：先合并，再对每段计 `hi - lo + 1` 求和（重叠只算一次）。

### 6. units/flags —— `Impl.parse(argv)`
- 非 `-` 开头或正好 `-` 的项按顺序进 `positional`。
- `--` 之后的全部项进 `positional`，`--` 自身不出现。
- `--key=value` 与 `--key value` 均为键值对。
- `--key` 后无可用值（到结尾，或下一项以 `--` 开头）时作为开关，值为 `True`；
  注意下一项是 `-5` 这类单项仍作为值。
- `-abc` 展开为开关 `a`、`b`、`c`。
- `values` 每个键是列表、重复按键出现顺序累积；重复开关仍为 `True`。

### 7. units/measure —— `Impl.to_cm(value, unit)`
- `m -> value*100`，`cm -> value`，`mm -> value*0.1`（数值比较兼容 `3.0 == 3`）。
- 其它单位抛 `ValueError`。

### 8. units/slugify —— `Impl.slugify(text)`
- 小写化；把**连续的非字母数字**替换为单个 `-`；去掉首尾 `-`；
  结果为空（如 `"***"`）返回 `"untitled"`。

### 9. units/ranges —— `Impl.merge(ranges)`
- 合并**互相交叠**的闭区间（端点相接也合并），返回按起点升序的**元组列表**。
- 仅空隙而没有交叠的区间保持分离（`[[5,7],[1,3],[2,4]] -> [(1,4),(5,7)]`）。
- 空输入 `[]`。

### 10. units/alpha —— `alpha.py`
- `add(a, b)` 修正为 `a + b`（原为 `a - b`）。`check.py` 未改。

### 11. units/beta —— `beta.py`
- `mul(a, b)` 修正为 `a * b`（原为 `a + b`）。`check.py` 未改。

### 12. units/chain —— `stage1..stage4`
- `stage1.parse_orders(path)`：`csv.DictReader` 读取，字段 `id/qty/price`
  转 `int`，返回字典列表。
- `stage2.filter_orders(rows)`：仅保留 `qty > 0` 的行。
- `stage3.total_by_price(rows)`：按 `price` 汇总 `qty`，返回 `{price: qty}`。
- `stage4.report(totals)`：按 `price` 升序渲染 `"{price}:{qty}\n"` 拼接文本。

## 改动文件清单
- `units/csvfix/impl.py`
- `units/rules/engine.py`
- `units/ledger/ledger.py`
- `units/schedule/schedule.py`
- `units/intervals/intervals.py`
- `units/flags/flags.py`
- `units/measure/measure.py`
- `units/slugify/slugify.py`
- `units/ranges/ranges.py`
- `units/alpha/alpha.py`
- `units/beta/beta.py`
- `units/chain/stage1.py`、`stage2.py`、`stage3.py`、`stage4.py`
- `REPORT.md`（本文件）

未改动的验收/测试文件：`units/*/test_*.py`、`units/chain/tests/test_*.py`、
`units/alpha/check.py`、`units/beta/check.py`。
