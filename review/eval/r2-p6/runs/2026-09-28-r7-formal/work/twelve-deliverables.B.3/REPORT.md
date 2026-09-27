# REPORT — twelve-deliverables.B.3

12 个互相独立的交付物，全部实现并真实验证通过。未修改任何 `test_*.py` / `check.py` / 验收文件。

## 运行环境

- Python 3.13.15，pytest 9.0.3
- 工作目录：`twelve-deliverables.B.3/`

## 1. `units/csvfix/` — `impl.parse` / `impl.total`

- `parse(line)`：按 CSV 规则解析单行（双引号包裹的字段内的逗号不作为分隔符），每个字段 `strip()`；列数不等于 3 返回 `None`。
- `total(rows)`：每行 `[name, qty, price]`，金额按 `qty * price` 计算后求和；`qty` 或 `price` 为空字符串的行跳过；非空但非整数的字段抛 `ValueError`；列数不为 3 抛 `ValueError`。
  - 语义边界/解释：原 docstring 指出两处 bug「空字段当作 0 不报错」「把第三列当成金额」。因此金额不是直接取第三列（price），而是 `qty*price`（测试 `[["a","1","2"],["b","2",""]] -> 2` 与 `1*2` 一致）。
- 验证：`cd units/csvfix && python3 -m pytest -q` → `2 passed`。

## 2. `units/rules/` — `engine.evaluate`

- `{"all":[...]}`：所有键 `facts.get(k)` 为真才返回 `True`（空列表视为全真）。
- `{"any":[...]}`：任一键为真返回 `True`（空列表视为假）。
- 其它形状返回 `False`；返回真正的 `bool`（测试用 `is True/False` 判定）。
- 验证：`1 passed`。

## 3. `units/ledger/` — `Ledger.apply` / `balance`

- `apply(rows, op)`：先校验所有行，`kind` 只能是 `deposit`/`withdraw`（否则 `ValueError`），`cents < 0` 抛 `ValueError`；然后返回 `account == op` 的行，保持原顺序。
- `balance(rows, account)`：只统计该账户，`deposit` 加、`withdraw` 减；无该账户返回 `0`。
- 验证：`2 passed`。

## 4. `units/schedule/` — `slots` / `overlaps`

- `slots(ranges, minutes)`：按起点排序合并闭区间；当 `next_lo - prev_hi < minutes` 视为同段（含重叠，`hi` 取最大）。空输入 `[]`，返回按起点升序的元组列表。`(0,90)` 与 `(120,150)` 的空隙 `120-90=30` 不小于 `30`，故不合并。
- `overlaps(a,b)`：端点恰好相接（`a.hi==b.lo` 或 `b.hi==a.lo`）返回 `False`；否则 `a.lo<=b.hi and b.lo<=a.hi`。
- 验证：`2 passed`。

## 5. `units/intervals/` — `Impl.merge/subtract/total_length`

- `merge(ranges, gap=0)`：先按 `lo` 排序（并对反向端点做 min/max 归一），相邻缺失整数数 `next.lo - prev.hi - 1 <= gap` 时合并，`hi` 取最大。空输入 `[]`。
- `subtract(ranges, hole)`：先 `merge` 归一化；与 `hole` 不相交的段保留；被覆盖的段消失；横跨的段按 `lo..hlo-1` 与 `hhi+1..hi` 分裂。
- `total_length(ranges)`：`merge` 后对每个区间累加 `hi-lo+1`（重叠只算一次）。
- 验证：`7 passed`。

## 6. `units/flags/` — `Impl.parse`

- 逐项扫描 `argv`：`--` 之后的项全部进 `positional`（`--` 自身不出现）。
- `--key=value` → `values[key].append(value)`；`--key value`：仅当下一个项存在且**不以 `--` 开头**时取作值，否则 `flags[key]=True`（`--key` 后是另一个 `--` 或到结尾都算开关）。
- `-abc` → `flags` 中 `a/b/c` 各为 `True`；`-` 恰好一项为位置参数。
- `values` 的值为列表，重复键按出现顺序累积；重复开关仍为 `True`。
- 边界：`--k -5` 中 `-5` 不以 `--` 开头，作为值；`--k=--v` 中 `--v` 作为值。
- 验证：`8 passed`。

## 7. `units/measure/` — `Impl.to_cm(value, unit)`

- `m→×100`，`cm→×1`，`mm→×0.1`；单位不在 `{m,cm,mm}` 时抛 `ValueError`。
- 验证：`1 passed`。

## 8. `units/slugify/` — `Impl.slugify(text)`

- 先 `lower()`；连续的非字母数字折叠成单个 `-`；去掉首尾 `-`；结果为空返回 `"untitled"`。用 `str.isalnum()` 判定字母数字（兼容非 ASCII）。
- 验证：`1 passed`。

## 9. `units/ranges/` — `Impl.merge(ranges)`

- 按起点排序，仅当 `lo <= 当前段.hi`（真正重叠）才合并，`hi` 取最大；恰好相邻（`lo == hi+1`）不合并。返回按起点排序的元组列表；空输入 `[]`。
- 与 `intervals.merge` 的差异：本模块默认 `gap` 语义下相邻不合并，`[[1,3],[2,4],[5,7]] -> [(1,4),(5,7)]`。
- 验证：`1 passed`。

## 10. `units/alpha/` — `alpha.add`

- 修复 `return a - b` → `return a + b`。
- 验证：`cd units/alpha && python3 check.py` → `alpha ok`。

## 11. `units/beta/` — `beta.mul`

- 修复 `return a + b` → `return a * b`。
- 验证：`cd units/beta && python3 check.py` → `beta ok`。

## 12. `units/chain/` — `stage1..stage4`

- `stage1.parse_orders(path)`：`csv.DictReader` 读取，`id/qty/price` 转 `int`，返回 dict 列表。
- `stage2.filter_orders(rows)`：保留 `qty > 0`，顺序不变。
- `stage3.total_by_price(rows)`：按 `price` 汇总 `qty`，返回 `{price: total_qty}`。
- `stage4.report(totals)`：按 `price` 升序渲染 `"price:total_qty\n"`。
- 端到端：`data/orders.csv` → `report(...)` = `"7:4\n10:2\n"`。
- 验证：`cd units/chain && python3 -m pytest -q tests/` → `4 passed`。

## 真实运行过的命令与结果

```
# 逐块
cd units/csvfix    && python3 -m pytest -q        -> 2 passed
cd units/rules     && python3 -m pytest -q        -> 1 passed
cd units/ledger    && python3 -m pytest -q        -> 2 passed
cd units/schedule  && python3 -m pytest -q        -> 2 passed
cd units/intervals && python3 -m pytest -q        -> 7 passed
cd units/flags     && python3 -m pytest -q        -> 8 passed
cd units/measure   && python3 -m pytest -q        -> 1 passed
cd units/slugify   && python3 -m pytest -q        -> 1 passed
cd units/ranges    && python3 -m pytest -q        -> 1 passed
cd units/alpha     && python3 check.py            -> "alpha ok"
cd units/beta      && python3 check.py            -> "beta ok"
cd units/chain     && python3 -m pytest -q tests/ -> 4 passed
```

汇总：12/12 交付物的验收命令全部通过（合计 30 个 pytest 用例 + 2 个 check 脚本）。

## 修改的文件

- `units/csvfix/impl.py`（实现 parse/total）
- `units/rules/engine.py`（实现 evaluate）
- `units/ledger/ledger.py`（实现 apply/balance）
- `units/schedule/schedule.py`（实现 slots/overlaps）
- `units/intervals/intervals.py`（实现 merge/subtract/total_length）
- `units/flags/flags.py`（实现 parse）
- `units/measure/measure.py`（实现 to_cm）
- `units/slugify/slugify.py`（实现 slugify）
- `units/ranges/ranges.py`（实现 merge）
- `units/alpha/alpha.py`（修 add）
- `units/beta/beta.py`（修 mul）
- `units/chain/stage1.py` … `stage4.py`（实现四个阶段）
- `REPORT.md`（本文件）

## 未验证 / 语义存疑

- `csvfix.total` 的金额列语义存在歧义：docstring 说「把第三列当成金额」是 bug，我据此解释为 `qty*price`（第三个测试样例 `1*2=2` 也吻合）。若验收方原意是直接取第三列求和，则该函数需改为直接累加第三列。
- 其余单元的边界（如 `flags` 中 `--k -v`、`intervals` 的反向区间）按 docstring 实现，但冻结测试未覆盖这些点。
