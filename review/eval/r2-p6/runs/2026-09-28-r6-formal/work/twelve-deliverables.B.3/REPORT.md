# 12 个独立交付物 — 实现报告

日期：2026-09-24

所有 12 个单元互相独立完成。**没有修改任何测试/验收文件**（`*/test_*.py`、
`chain/tests/*`、`alpha/check.py`、`beta/check.py` 均保持原样），只写了实现文件。
测试语义以各目录文档字符串 + 冻结测试为准。

## 验收命令与真实结果

在仓库根目录 `twelve-deliverables.B.3/` 逐一执行，全部通过：

| # | 单元 | 命令 | 结果 |
|---|------|------|------|
| 1 | csvfix | `cd units/csvfix && python3 -m pytest -q` | 2 passed |
| 2 | rules | `cd units/rules && python3 -m pytest -q` | 1 passed |
| 3 | ledger | `cd units/ledger && python3 -m pytest -q` | 2 passed |
| 4 | schedule | `cd units/schedule && python3 -m pytest -q` | 2 passed |
| 5 | intervals | `cd units/intervals && python3 -m pytest -q` | 7 passed |
| 6 | flags | `cd units/flags && python3 -m pytest -q` | 8 passed |
| 7 | measure | `cd units/measure && python3 -m pytest -q` | 1 passed |
| 8 | slugify | `cd units/slugify && python3 -m pytest -q` | 1 passed |
| 9 | ranges | `cd units/ranges && python3 -m pytest -q` | 1 passed |
| 10 | alpha | `cd units/alpha && python3 check.py` | 输出 `alpha ok`，exit 0 |
| 11 | beta | `cd units/beta && python3 check.py` | 输出 `beta ok`，exit 0 |
| 12 | chain | `cd units/chain && python3 -m pytest -q tests/` | 4 passed |

环境：Python 3.13.15，pytest 9.0.3。

## 逐块语义与边界

### 1. `units/csvfix/impl.py` — `parse` / `total`
- `parse(line)`：用 `csv.reader` 解析单行 CSV，支持双引号字段（内部可含逗号）；
  对每个字段 `strip()`；**字段数必须正好 3**，否则返回 `None`（不抛异常）。
  非字符串输入也返回 `None`。
- `total(rows)`：每行 `[id, qty, price]`，返回 `Σ qty*price`。
  - `price` 为空白字符串的行视为无金额，跳过；行长度 < 3 跳过。
  - `qty`/`price` 非空但无法转成整数时抛 `ValueError`（不再静默当 0）。
  - **语义说明（重要）**：文档字符串把「把第三列当成金额」列为 bug 之一。
    冻结测试 `total([["a","1","2"],["b","2",""]]) == 2` 在「直接求和第三列（2+跳过=2）」
    与「数量\*单价（1\*2+跳过=2）」两种读法下都成立。按文档字符串（第三列是单价而非金额），
    这里采用 **qty \* price** 的读法，两种读法都能通过该冻结测试。
- 实测：`parse('"a,b",2,x') == ['a,b','2','x']`；`parse('a,2') is None`；`parse('a,b,c,d') is None`。

### 2. `units/rules/engine.py` — `evaluate(rules, facts)`
- `{"all": [k...]}` → 全部命中为真才 `True`；`{"any": [k...]}` → 任一命中即 `True`。
- 缺失的 key 视为假（`facts.get(k, False)`）；非法规则返回 `False`。
- `all`/`any` 保证返回真正的 `bool`（满足 `is True/False` 断言）。

### 3. `units/ledger/ledger.py` — `Ledger.apply` / `balance`
- `apply(rows, op)`：`op` 是要筛选的账户名；返回该账户的行（保持原顺序）。
  kind 只允许 `deposit`/`withdraw`，否则 `ValueError`；金额为负抛 `ValueError`（且先校验后筛选）。
- `balance(rows, account)`：deposit 加、withdraw 减；账户不存在返回 `0`；未知 kind 抛 `ValueError`。

### 4. `units/schedule/schedule.py` — `slots` / `overlaps`
- `slots(ranges, minutes)`：闭区间按 `(lo, hi)` 排序后合并；相邻空隙 `next_lo - prev_hi < minutes`
  视为同一段（**严格小于**，空隙正好等于 `minutes` 时保持分开）；区间重叠（差为负）必然合并；
  合并时 `hi` 取 `max`。空输入返回 `[]`，返回元组列表。
- `overlaps(a, b)`：`a.lo < b.hi and b.lo < a.hi`；端点相接（`(0,10)` 与 `(10,20)`）不算交叠。

### 5. `units/intervals/intervals.py` — `Impl.merge/subtract/total_length`
- `merge(ranges, gap=0)`：按 `lo` 排序；相邻两段**缺失整数个数** `next.lo - prev.hi - 1 <= gap`
  才合并；处理包含/嵌套（`hi` 取 max）；空输入返回 `[]`。
- `subtract(ranges, hole)`：先用 `merge` 规范化输入，再逐段挖洞；不交叠原样保留，完全覆盖消失，
  横跨时按 `hlo-1` / `hhi+1` 分裂；返回升序闭区间。
  实测 `subtract([[5,7],[1,3]], (2,6)) == [(1,1),(7,7)]`。
- `total_length(ranges)`：先 `merge` 去重，再 `Σ (hi-lo+1)`；空输入为 `0`。

### 6. `units/flags/flags.py` — `Impl.parse`
返回 `{"values": {...}, "flags": {...}, "positional": [...]}`：
- 非 `-` 开头或正好 `-` 的项进 `positional`；`--` 之后所有项进 `positional`（`--` 本身不出现）。
- `--key=value` 与 `--key value`（下一个项不以 `--` 开头）进 `values`，值为列表、按出现顺序累积。
- `--key` 到结尾或下一项以 `--` 开头 → 开关 `flags[key] = True`。
- `-abc` 拆成三个开关 a/b/c；`--k -5` 因 `-5` 不以 `--` 开头而当作值 `"-5"`（按文档）。
- 开关重复出现仍为 `True`。

### 7. `units/measure/measure.py` — `Impl.to_cm`
- `m`→×100，`cm`→×1，`mm`→×0.1；其它单位抛 `ValueError`。
- 返回数值（`1.5 m -> 150.0`，`20 mm -> 2.0`，与整数相等比较通过）。

### 8. `units/slugify/slugify.py` — `Impl.slugify`
- 先 NFKD 分解并丢弃组合符号（`café -> cafe`，非 ASCII 会被丢弃），再小写化。
- 连续非 `[a-z0-9]` 折叠成单个 `-`；去掉首尾 `-`；结果为空返回 `"untitled"`。
- 实测 `"Café  Déjà vu!" -> "cafe-deja-vu"`，`"***" -> "untitled"`。

### 9. `units/ranges/ranges.py` — `Impl.merge`
- 按起点排序闭区间；仅当 `lo <= 当前 hi`（有交叠，含共享端点）才合并，`hi` 取 max；
  相邻但有真实空隙的区间（`(1,4)` 与 `(5,7)`）保持分开；空输入 `[]`；返回元组列表。

### 10. `units/alpha/alpha.py`
- `add(a, b)` 由 `a - b` 修正为 `a + b`。未改 `check.py`。

### 11. `units/beta/beta.py`
- `mul(a, b)` 由 `a + b` 修正为 `a * b`。未改 `check.py`。

### 12. `units/chain/stage1..4`
- `stage1.parse_orders(path)`：`csv.DictReader` 读文件，返回
  `[{"id": int, "qty": int, "price": int}, ...]`。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行，顺序不变。
- `stage3.total_by_price(rows)`：按 `price` 分组累加 `qty`，返回 `{price: total_qty}`。
- `stage4.report(totals)`：按 `price` 升序渲染 `"price:qty\n"`。
- 实测 `report({10:2, 7:4}) == "7:4\n10:2\n"`。

## 备注 / 未验证项
- csvfix 的 `total` 采用 `qty*price` 语义（见上，依据文档字符串），该选择通过了冻结测试，
  但冻结测试本身无法区分「求和第三列」，故不对「求和第三列」这一替代读法做断言。
- 未运行任务清单之外的额外/隐藏验收；报告中所有结果均为上表命令的真实输出。
