# 12 个独立交付物 — 实现报告

日期：2026-09-24

本报告逐块说明实现的语义边界，以及真实运行过的命令与结果。所有验收测试/检查脚本均未修改。

## 总览：运行过的命令与结果

验收命令（工作目录为各单元目录，结果均为最后一行）：

| 单元 | 命令 | 结果 |
| --- | --- | --- |
| csvfix | `cd units/csvfix && python3 -m pytest -q` | `2 passed` |
| rules | `cd units/rules && python3 -m pytest -q` | `1 passed` |
| ledger | `cd units/ledger && python3 -m pytest -q` | `2 passed` |
| schedule | `cd units/schedule && python3 -m pytest -q` | `2 passed` |
| intervals | `cd units/intervals && python3 -m pytest -q` | `7 passed` |
| flags | `cd units/flags && python3 -m pytest -q` | `8 passed` |
| measure | `cd units/measure && python3 -m pytest -q` | `1 passed` |
| slugify | `cd units/slugify && python3 -m pytest -q` | `1 passed` |
| ranges | `cd units/ranges && python3 -m pytest -q` | `1 passed` |
| alpha | `cd units/alpha && python3 check.py` | 输出 `alpha ok`，退出码 0 |
| beta | `cd units/beta && python3 check.py` | 输出 `beta ok`，退出码 0 |
| chain | `cd units/chain && python3 -m pytest -q tests/` | `4 passed` |

修改的文件（仅实现文件，未触碰测试/验收文件）：

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

---

## 1. `units/csvfix/` — `impl.parse` / `impl.total`

`parse(line)`：
- 用 `csv.reader` 解析，支持双引号包裹的字段（引号内逗号不分隔，`""` 表示字面引号）。
- 对每个字段做 `strip()`（去首尾空白）。
- 字段数不是 3 时返回 `None`；解析异常也返回 `None`。
- 边界：空行 → `[]` → 长度 0 → `None`。

`total(rows)`：
- 行格式约定为 `[label, qty, price]`，结果为 `Σ qty * price`。
- 任一数值字段为空字符串的行整行跳过（不把空字段当成 0）。
- 非空但无法转成整数的数值字段会抛 `ValueError`（"跳过空白、报错坏值"）。
- 边界确认：`total([["a","1","2"],["b","2",""]]) == 2`（第二行 price 为空，跳过）。
- 说明：原注释同时指出"空字段当成 0 不报错"与"把第三列当成金额"两个问题；冻结测试要求空字段行被跳过而非抛错，因此本实现按测试语义处理（跳过空字段行、对非空坏值抛 `ValueError`），并把金额改为 `qty*price`。

## 2. `units/rules/engine.py` — `evaluate`

- `{"all": [k,...]}`：所有 key 在 facts 中为真才为 `True`；空列表为 `True`（`all([])` 语义）。
- `{"any": [k,...]}`：任一为真即为 `True`；空列表为 `False`。
- 缺失 key 视为 `False`；既非 `all` 也非 `any` 的规则返回 `False`。
- 返回值为真正的 `bool`，满足测试中的 `is True` / `is False`。

## 3. `units/ledger/ledger.py` — `Ledger.apply` / `balance`

`Ledger.apply(rows, op)`：
- 输入 `(kind, account, cents)` 元组序列，返回 `account == op` 的行，保持原顺序，元素仍为元组。
- 任一行 `cents < 0` 即抛 `ValueError`（对全部输入行校验，而不仅是目标账户）。
- 未对未知 `kind` 做校验抛错（文档只声明 kind 取值域），`balance` 中未知 kind 不参与计算。

`balance(rows, account)`：
- 只累加 `account` 的行；`deposit` 加、`withdraw` 减。
- 没有该账户的行时返回 `0`。

## 4. `units/schedule/schedule.py` — `slots` / `overlaps`

`slots(ranges, minutes)`：
- 先按 `(lo, hi)` 升序排序；空输入返回 `[]`。
- 相邻两段的空隙 `next.lo - prev.hi` **严格小于** `minutes` 时合并，合并后取 `max(hi)` 处理嵌套。
- 空隙正好等于 `minutes` 时不合并（测试 `(0,90)` 与 `(120,150)` 间隙 30，保持分开）。
- 返回升序的元组列表。

`overlaps(a, b)`：
- 闭区间交叠判据为 `a.lo < b.hi and b.lo < a.hi`；端点相接（如 `(0,10)` 与 `(10,20)`）返回 `False`。

## 5. `units/intervals/intervals.py` — `Impl.merge` / `subtract` / `total_length`

`merge(ranges, gap=0)`：
- 按 `(lo, hi)` 升序排序；空输入返回 `[]`。
- 相邻两段"缺失的整数个数"为 `next.lo - prev.hi - 1`，`<= gap` 即合并；合并取 `max(hi)` 处理嵌套。
- 返回值为元组列表（输入可以是 list 或 tuple）。
- 边界：`gap=0` 时相邻整数（缺失 0 个）合并，如 `[1,2],[3,4] → (1,4)`；缺失 ≥1 个则保留空隙。

`subtract(ranges, hole)`：
- 先用 `merge(ranges)` 归一化输入，再逐段挖去 `hole`。
- 与 `hole` 不相交（`hole.hi < lo` 或 `hole.lo > hi`）的段原样保留。
- 相交时保留左侧 `(lo, hole.lo-1)` 与右侧 `(hole.hi+1, hi)`，空段不产生。
- 端点相接也算相交：如 `subtract([[0,4]], (4,9)) == [(0,3)]`。

`total_length(ranges)`：
- `merge` 归一化后求和 `Σ(hi - lo + 1)`，重叠区间只算一次。空输入为 `0`。

## 6. `units/flags/flags.py` — `Impl.parse`

返回 `{"values", "flags", "positional"}`：
- 不以 `-` 开头、或正好等于 `-` 的 token 进 `positional`（保持出现顺序）。
- 遇到 `--` 后，其余全部进 `positional`，`--` 自身不出现。
- `--key=value`：按第一个 `=` 切分，`value` 可含 `=` / 以 `--` 开头（如 `--k=--v`）。
- `--key value`：后一个 token 不以 `--` 开头即作为值；可用值可以是 `-5` 这类单横线 token。
- `--key` 后面没有可用值（到结尾或下一 token 以 `--` 开头）时作为开关，`flags[key]=True`。
- `-abc`：拆成开关 `a`、`b`、`c`。
- `values` 每个键的值是列表，重复键按出现顺序累积；开关重复出现仍为 `True`。
- 同一键可同时出现在 `values` 与 `flags`（如 `["--x","y","--x"]`）。

## 7. `units/measure/measure.py` — `Impl.to_cm`

- 换算表：`m → ×100`、`cm → ×1`、`mm → ×0.1`。
- 未知单位（如 `ft`）抛 `ValueError`；不可哈希的 unit 也抛 `ValueError`。
- 返回值为数值（`to_cm(1.5,"m")==150`、`to_cm(20,"mm")==2`）。

## 8. `units/slugify/slugify.py` — `Impl.slugify`

- 先 `lower()`，再逐字符：字母数字（`str.isalnum()`）保留，连续的非字母数字折叠成单个 `-`。
- 去掉首尾 `-`；结果为空时返回 `"untitled"`。
- 边界：`"***"` → `"untitled"`；`"  --A@@B-- "` → `"a-b"`。

## 9. `units/ranges/ranges.py` — `Impl.merge`

- 按 `(lo, hi)` 升序排序；空输入返回 `[]`。
- 相邻两段满足 `next.lo <= prev.hi`（有交叠）即合并，合并取 `max(hi)`；返回元组列表。
- 注意与本仓库 `intervals.merge` 的差别：`ranges` 按交叠合并，`(1,4)` 与 `(5,7)` 保留为两段（不按整数相邻合并）。

## 10. `units/alpha/alpha.py` — `add`

- 修正符号：`return a + b`（原为 `a - b`）。`check.py` 未改动。

## 11. `units/beta/beta.py` — `mul`

- 修正运算符：`return a * b`（原为 `a + b`）。`check.py` 未改动。

## 12. `units/chain/` — `parse_orders` / `filter_orders` / `total_by_price` / `report`

`stage1.parse_orders(path)`：
- 读取表头为 `id,qty,price` 的 CSV，按文件行序返回 `{"id": int, "qty": int, "price": int}` 列表。
- 相对路径 `data/orders.csv` 依赖从 `units/chain` 作为工作目录运行。

`stage2.filter_orders(rows)`：
- 保留 `qty > 0` 的行，保持原顺序。

`stage3.total_by_price(rows)`：
- 按 `price` 汇总 `qty`，返回 `{price: Σqty}`。

`stage4.report(totals)`：
- 按 price 升序渲染为 `"price:qty\n"` 拼接的文本，例如 `{10:2,7:4}` → `"7:4\n10:2\n"`。

---

## 额外的边界验证（非验收命令，仅自检）

在一个 `python3` 进程中额外断言/打印了以下结果，全部符合预期：

- `parse('"a,b", 2 , x')` → `['a,b', '2', 'x']`；`parse('a,b')` → `None`。
- `intervals.merge([[0,10],[2,3]])` → `[(0,10)]`；`merge([[1,2],[4,5]],1)` → `[(1,5)]`。
- `intervals.subtract([[0,4]],(4,9))` → `[(0,3)]`；`total_length([[0,2],[2,4]])` → `5`。
- `flags.parse(["--","--not-a-flag","x"])` → `{"values":{},"flags":{},"positional":["--not-a-flag","x"]}`。
- `flags.parse(["--k","-5"])` → `{"values":{"k":["-5"]},"flags":{},"positional":[]}`。
- `schedule.slots([(0,60),(30,90),(120,150)],30)` → `[(0,90),(120,150)]`。
- `schedule.overlaps((0,10),(10,20))` → `False`；`overlaps((0,10),(5,20))` → `True`。

## 未验证/说明

- `units/csvfix` 的 `total` 金额列语义存在文档与冻结测试的张力：文档把"第三列当金额"列为 bug，而冻结测试中唯一含两个数值列的样本无法区分"第三列"与"qty*price"两种实现。本实现选择 `qty*price` 并跳过空字段行（以通过冻结测试为准），已在第 1 节说明。
- `ledger` 未对未知 `kind` 抛错、`slugify` 的"字母数字"采用 Unicode `str.isalnum()` 语义，这两点均无对应验收测试，属实现选择。
