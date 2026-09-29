# 12 独立交付物 —— 实现报告

本报告覆盖 `units/` 下的 12 个互相独立的交付物。每块由一个独立 worker 实现，随后由集成者（leader）在同一工作区用各块规定的验收命令**重新真实运行**确认。

- 约定：未修改任何测试/验收文件（`test_*.py`、`check.py`、`units/chain/tests/`、`units/chain/data/`）。
- 环境：Python 3.13.15，pytest 9.0.3，Linux。
- 集成复跑时间：2026-09-29（会话日期），结果如下。

## 总览（集成者实际运行结果）

| # | 单元 | 验收命令 | 结果 |
|---|------|----------|------|
| 1 | csvfix | `cd units/csvfix && python3 -m pytest -q` | `2 passed in 0.00s` (exit 0) |
| 2 | rules | `cd units/rules && python3 -m pytest -q` | `1 passed in 0.00s` (exit 0) |
| 3 | ledger | `cd units/ledger && python3 -m pytest -q` | `2 passed in 0.00s` (exit 0) |
| 4 | schedule | `cd units/schedule && python3 -m pytest -q` | `2 passed in 0.00s` (exit 0) |
| 5 | intervals | `cd units/intervals && python3 -m pytest -q` | `7 passed in 0.00s` (exit 0) |
| 6 | flags | `cd units/flags && python3 -m pytest -q` | `8 passed in 0.00s` (exit 0) |
| 7 | measure | `cd units/measure && python3 -m pytest -q` | `1 passed in 0.00s` (exit 0) |
| 8 | slugify | `cd units/slugify && python3 -m pytest -q` | `1 passed in 0.00s` (exit 0) |
| 9 | ranges | `cd units/ranges && python3 -m pytest -q` | `1 passed in 0.00s` (exit 0) |
| 10 | alpha | `cd units/alpha && python3 check.py` | 输出 `alpha ok` (exit 0) |
| 11 | beta | `cd units/beta && python3 check.py` | 输出 `beta ok` (exit 0) |
| 12 | chain | `cd units/chain && python3 -m pytest -q tests/` | `4 passed in 0.01s` (exit 0) |

## 逐块语义边界

### 1. `units/csvfix/impl.py`
- `parse(line)`：按 `,` 切分，对每个字段 `strip()`；列数不是恰好 3 时返回 `None`；否则返回 3 元素 `list[str]`。不处理引号（超出本验收范围）。
- `total(rows)`：累加**第 3 列**（`row[2]`）的整数值；第 3 列为空字符串时跳过；列数不足 3 的行忽略；空输入返回 `0`。返回 `int`。
- 注意：docstring 中"把第三列当成金额"为误导性描述，测试 `rows=[["a","1","2"],["b","2",""]] -> 2` 明确要求用第三列（第二列会得 3）。以测试为准。
- 修改文件：`units/csvfix/impl.py`。

### 2. `units/rules/engine.py`
- `evaluate(rules, facts)`：`{"all": [k...]}` 当且仅当所有键在 facts 中为真；`{"any": [k...]}` 当且仅当至少一个为真；缺失键视为假（`facts.get(k)`）；空列表遵循内置语义 `all([])==True`、`any([])==False`；两者皆无时抛 `ValueError`。
- 修改文件：`units/rules/engine.py`。

### 3. `units/ledger/ledger.py`
- `Ledger.apply(rows, op)`：若任意行金额为负，抛 `ValueError`；否则返回账户等于 `op` 的行，保持原顺序（`(kind, account, cents)` 元组）。
- `balance(rows, account)`：`deposit` 相加、`withdraw` 相减，返回该账户净额；无该账户行返回 `0`。
- 修改文件：`units/ledger/ledger.py`。

### 4. `units/schedule/schedule.py`
- `slots(ranges, minutes)`：按起点排序的闭区间合并；相邻段间隙 `next_lo - prev_hi` **严格小于** `minutes` 才合并（等于 minutes 不合并）；空输入返回 `[]`。
- `overlaps(a, b)`：`a[0] < b[1] and b[0] < a[1]`，端点相接不算交叠。
- 修改文件：`units/schedule/schedule.py`。

### 5. `units/intervals/intervals.py`
- `merge(ranges, gap=0)`：按 `lo` 排序；相邻"缺失整数个数" `next.lo - prev.hi - 1 <= gap` 时合并，`hi` 取 `max` 以吸收嵌套区间；返回升序元组列表，空输入 `[]`。
- `subtract(ranges, hole)`：先 `merge` 规范化输入，再挖去闭区间 `hole`；不相交段保留，完全覆盖段消失，跨越段分裂；端点相触会真正去掉共享整数（`[0,4] - (4,9) -> [(0,3)]`）。
- `total_length(ranges)`：合并后对每段累加 `hi - lo + 1`，重叠只算一次；空输入 `0`。
- 修改文件：`units/intervals/intervals.py`。

### 6. `units/flags/flags.py`
- `Impl.parse(argv)` 返回 `{"values": {...}, "flags": {...}, "positional": [...]}`。
- 位置参数：不以 `-` 开头或恰为 `-` 的项按序入 `positional`；`--` 之后所有项均入 `positional`（`--` 本身不出现）。
- `--key=value` 与 `--key value` 均产生键值对；`--key` 后无可用值（到结尾，或下一项以 `--` 开头）时记为开关。
- `-abc` 展开为开关 `a`、`b`、`c`。
- `values` 每个键对应列表，重复键按出现顺序累积；开关重复仍为 `True`。
- 边界：`--k -5` 中 `-5` 可作为值；`--k=--v` 的值为 `--v`。
- 修改文件：`units/flags/flags.py`。

### 7. `units/measure/measure.py`
- `Impl.to_cm(value, unit)`：`m -> value*100`，`cm -> value`，`mm -> value/10`；其他单位抛 `ValueError`。
- 修改文件：`units/measure/measure.py`。

### 8. `units/slugify/slugify.py`
- `Impl.slugify(text)`：小写化；把连续非字母数字字符折叠为单个 `-`；去掉首尾 `-`；结果为空返回 `"untitled"`。
- 修改文件：`units/slugify/slugify.py`。

### 9. `units/ranges/ranges.py`
- `Impl.merge(ranges)`：排序后贪心合并重叠、相接（`start <= prev_end`）或嵌套的闭区间，`end` 取 `max`；返回按起点排序的元组列表；空/`None` 输入返回 `[]`。
- 修改文件：`units/ranges/ranges.py`。

### 10. `units/alpha/alpha.py`
- `add` 原为 `a - b`（bug），改为 `a + b`。
- 修改文件：`units/alpha/alpha.py`。

### 11. `units/beta/beta.py`
- `mul` 原为 `a + b`（bug），改为 `a * b`。
- 修改文件：`units/beta/beta.py`。

### 12. `units/chain/`
- `stage1.parse_orders(path)`：`csv.DictReader` 读文件，跳过表头，`id/qty/price` 转 `int`，返回 dict 列表。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行，保持顺序。
- `stage3.total_by_price(rows)`：按 `price` 汇总 `qty`，返回 `{price: total_qty}`。
- `stage4.report(totals)`：按价格升序生成 `"{price}:{total}\n"` 拼接的字符串。
- 修改文件：`units/chain/stage1.py`、`stage2.py`、`stage3.py`、`stage4.py`。

## 未改动 / 未验证

- 未改动：所有 `test_*.py`、`check.py`、`units/chain/tests/`、`units/chain/data/`。
- 各块只保证"规定验收命令通过"及测试/文档字符串覆盖到的语义；未对验收范围之外的输入（如 csvfix 的引号、flags 的长选项缩写、非整数端点等）做额外保证。
- 集成复跑仅覆盖上表命令；8 个 pytest 单元未做交叉依赖测试（12 块声明为互相独立）。
