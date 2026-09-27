# 12 个独立交付物 —— 实现与验证报告

所有实现只在各单元的 `impl.py` / `engine.py` / `ledger.py` / `schedule.py` / `intervals.py` / `flags.py` /
`measure.py` / `slugify.py` / `ranges.py` / `alpha.py` / `beta.py` / `chain/stage*.py` 中进行；
**未改动任何测试或验收文件**（`test_*.py`、`check.py` 保持原样）。

## 逐块语义边界

### 1. `units/csvfix/` — `impl.parse` / `impl.total`
- `parse(line)`：用 `csv.reader` 解析单行，字段逐个 `strip()`，因此支持引号字段与字段内逗号；
  列数恰好为 3 时返回 `["a","2","x"]` 这样的列表，否则返回 `None`（引号不闭合等解析错误同样返回 `None`）。
- `total(rows)`：累加每行**第三列**（金额），空字符串跳过；列数不为 3 或金额非整数时抛 `ValueError`。
- 验收：`cd units/csvfix && python3 -m pytest -q` → **2 passed**。

### 2. `units/rules/` — `engine.evaluate`
- 支持 `{"all": [...]}`：所列键全部为真才返回 `True`（空 all 列表为 `True`）。
- 支持 `{"any": [...]}`：任一为真即 `True`（空 any 列表为 `False`）。
- 缺省键按 `False` 处理；两个键同时出现时需同时满足；没有任何已知键时返回 `False`。
- 验收：`cd units/rules && python3 -m pytest -q` → **1 passed**。

### 3. `units/ledger/` — `Ledger.apply` / `balance`
- `apply(rows, op)`：遍历输入，校验 `kind ∈ {deposit, withdraw}`、金额非负（否则 `ValueError`），
  返回 `account == op` 的行，保持原顺序，元素仍是三元组。
- `balance(rows, account)`：过滤该账户，`deposit` 相加、`withdraw` 相减；无该账户的行返回 `0`。
- 验收：`cd units/ledger && python3 -m pytest -q` → **2 passed**。

### 4. `units/schedule/` — `slots` / `overlaps`
- `slots(ranges, minutes)`：按起点排序，若 `next_lo - prev_hi < minutes`（或已重叠）则合并，
  否则另起一段；返回按起点升序的元组列表，空输入返回 `[]`。空隙等于 `minutes` 不合并。
- `overlaps(a, b)`：闭区间判交叠，用 `max(lo) < min(hi)`，因此端点相接（如 `(0,10)` 与 `(10,20)`）不算交叠。
- 验收：`cd units/schedule && python3 -m pytest -q` → **2 passed**。

### 5. `units/intervals/` — `Impl.merge` / `subtract` / `total_length`
- `merge(ranges, gap=0)`：按 `lo` 排序，`next.lo - prev.hi - 1 <= gap`（缺失整数个数）时合并，
  取 `max` 端点处理包含关系；返回升序元组列表，空输入 `[]`。
- `subtract(ranges, hole)`：先用 `merge` 规范化输入；不相交段原样保留，完全覆盖的段消失，
  横跨 `hole` 的段按 `hole_lo-1` / `hole_hi+1` 分裂。
- `total_length(ranges)`：先 `merge` 去重，再累加 `hi - lo + 1`。
- 验收：`cd units/intervals && python3 -m pytest -q` → **7 passed**。

### 6. `units/flags/` — `Impl.parse`
- 非 `-` 开头或正好 `-` 的项进 `positional`；`--` 之后全部为位置参数且 `--` 本身不出现。
- `--key=value` 与 `--key value` 都进 `values[key]`（列表，按出现顺序累积）。
- `--key` 后无可用值（结尾，或下一项以 `--` 开头）时为开关，`flags[key]=True`。
- `-abc` 展开为 `a`/`b`/`c` 三个开关；开关重复仍为 `True`。
- 值不因形似开关而被拒绝（如 `--k -5`、`--k=--v`）。
- 验收：`cd units/flags && python3 -m pytest -q` → **8 passed**。

### 7. `units/measure/` — `Impl.to_cm`
- `m` → `value * 100`；`cm` → `value`；`mm` → `value / 10`；其他单位抛 `ValueError`。
- 验收：`cd units/measure && python3 -m pytest -q` → **1 passed**。

### 8. `units/slugify/` — `Impl.slugify`
- 先 `lower()`，再用 `[^a-z0-9]+` 折叠为单个 `-`，`strip("-")`；结果为空时返回 `"untitled"`。
- 验收：`cd units/slugify && python3 -m pytest -q` → **1 passed**。

### 9. `units/ranges/` — `Impl.merge`
- 按 `lo` 排序，仅当 `next.lo <= cur.hi`（重叠或包含）时合并；**相邻但不重叠**的闭区间保持分离，
  返回按起点排序的元组列表，空输入 `[]`。
- 验收：`cd units/ranges && python3 -m pytest -q` → **1 passed**。

### 10. `units/alpha/` — `alpha.add`
- 修复为 `a + b`（原为 `a - b`）。
- 验收：`cd units/alpha && python3 check.py` → 输出 `alpha ok`。

### 11. `units/beta/` — `beta.mul`
- 修复为 `a * b`（原为 `a + b`）。
- 验收：`cd units/beta && python3 check.py` → 输出 `beta ok`。

### 12. `units/chain/` — `stage1`~`stage4`
- `stage1.parse_orders(path)`：读 CSV（表头 `id,qty,price`），三个字段转 `int`，返回字典列表。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行。
- `stage3.total_by_price(rows)`：按 `price` 分组累加 `qty`，返回 `{price: total_qty}`。
- `stage4.report(totals)`：按 `price` 升序渲染 `"price:qty\n"` 拼接文本。
- 验收：`cd units/chain && python3 -m pytest -q tests/` → **4 passed**。

## 真实运行过的命令与结果

在仓库根目录 `/…/twelve-deliverables.B.3` 下依次执行（全部在真实 shell 中运行）：

| # | 命令（工作目录） | 结果 |
|---|---|---|
| 1 | `python3 -m pytest -q`（units/csvfix） | 2 passed |
| 2 | `python3 -m pytest -q`（units/rules） | 1 passed |
| 3 | `python3 -m pytest -q`（units/ledger） | 2 passed |
| 4 | `python3 -m pytest -q`（units/schedule） | 2 passed |
| 5 | `python3 -m pytest -q`（units/intervals） | 7 passed |
| 6 | `python3 -m pytest -q`（units/flags） | 8 passed |
| 7 | `python3 -m pytest -q`（units/measure） | 1 passed |
| 8 | `python3 -m pytest -q`（units/slugify） | 1 passed |
| 9 | `python3 -m pytest -q`（units/ranges） | 1 passed |
| 10 | `python3 check.py`（units/alpha） | 输出 `alpha ok`，退出码 0 |
| 11 | `python3 check.py`（units/beta） | 输出 `beta ok`，退出码 0 |
| 12 | `python3 -m pytest -q tests/`（units/chain） | 4 passed |

12 块全部通过，汇总 `OVERALL_FAIL=0`。

## 说明
- 各单元相互独立，未共享代码，也未修改测试/验收文件。
- `units/chain/tests/` 下原有的 `__pycache__` 未清理，不影响结果。
