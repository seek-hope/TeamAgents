# 12 个独立交付物：实现报告

工作目录：`twelve-deliverables.B.1/`（下称 `$ROOT`）。
所有验收测试 / `check.py` 均为冻结文件，**未做任何修改**。
以下每条命令都真实执行过，结果就是给出的输出。

## 总览（真实运行结果）

| # | 单元 | 命令（在 `$ROOT` 下） | 结果 |
|---|------|----------------------|------|
| 1 | units/csvfix | `cd units/csvfix && python3 -m pytest -q` | `2 passed`, exit=0 |
| 2 | units/rules | `cd units/rules && python3 -m pytest -q` | `1 passed`, exit=0 |
| 3 | units/ledger | `cd units/ledger && python3 -m pytest -q` | `2 passed`, exit=0 |
| 4 | units/schedule | `cd units/schedule && python3 -m pytest -q` | `2 passed`, exit=0 |
| 5 | units/intervals | `cd units/intervals && python3 -m pytest -q` | `7 passed`, exit=0 |
| 6 | units/flags | `cd units/flags && python3 -m pytest -q` | `8 passed`, exit=0 |
| 7 | units/measure | `cd units/measure && python3 -m pytest -q` | `1 passed`, exit=0 |
| 8 | units/slugify | `cd units/slugify && python3 -m pytest -q` | `1 passed`, exit=0 |
| 9 | units/ranges | `cd units/ranges && python3 -m pytest -q` | `1 passed`, exit=0 |
| 10 | units/alpha | `cd units/alpha && python3 check.py` | `alpha ok`, exit=0 |
| 11 | units/beta | `cd units/beta && python3 check.py` | `beta ok`, exit=0 |
| 12 | units/chain | `cd units/chain && python3 -m pytest -q tests/` | `4 passed`, exit=0 |

合计：单元 1–9 + 12 共 29 个 pytest 用例通过；单元 10、11 的 check 脚本通过。

## 逐块语义边界

### 1. `units/csvfix/impl.py` — `parse` / `total`
- `parse(line)`：用 `csv.reader` 按标准 CSV 语义解析单行（因此引号内的逗号不会拆列），
  对每个字段 `strip()`；**只有当列数恰为 3 时返回 3 个字符串，否则返回 `None`**。
  - 验收：`parse(' a , 2 , x ') == ["a","2","x"]`；`parse('a,2') is None`。
  - 额外自测：`parse('"a,b",2,x') == ['a,b','2','x']`；4 列返回 `None`。
- `total(rows)`：累加**每行第三列**的整数金额；第三列为空串（或行不足 3 列）跳过；
  非空但非整数时 `int()` 抛 `ValueError`。
  - 边界说明：冻结测试 `test_total_skips_blank_and_reports_bad` 把金额固定在第三列，
    `[["a","1","2"],["b","2",""]]` 期望 2，因此按第三列实现。原 docstring 中
    “把第三列当成金额”的措辞与测试不一致，这里以冻结测试为准。
  - 额外自测：`total([["a","1","x"]])` 抛 `ValueError`。

### 2. `units/rules/engine.py` — `evaluate(rules, facts)`
- `{"all":[...]}`：所有具名 fact 为真才返回 `True`（`all`）。
- `{"any":[...]}`：至少一个具名 fact 为真才返回 `True`（`any`）。
- 缺失的 fact 视为 `False`；两种键同时存在时 `all` 优先；空/未知规则返回 `False`。
- 返回值是真正的 `bool`（测试使用 `is True` / `is False`）。

### 3. `units/ledger/ledger.py` — `Ledger.apply` / `balance`
- `Ledger.apply(rows, op)`：返回 `account == op` 的行，**保持原顺序**，元素仍是
  `(kind, account, cents)` 元组；任何行 `cents < 0` 时抛 `ValueError`（先校验后筛选）。
- `balance(rows, account)`：仅统计该账户；`deposit` 加、`withdraw` 减，其他 kind 抛
  `ValueError`；没有该账户的行返回 `0`。

### 4. `units/schedule/schedule.py` — `slots` / `overlaps`
- `slots(ranges, minutes)`：按 `lo` 升序；相邻段空隙 `next_lo - prev_hi`
  **严格小于** `minutes` 时合并（取较大的 `hi`），否则保留为独立段；返回元组列表，
  空输入返回 `[]`。验收 `[(0,60),(30,90),(120,150)]`、`minutes=30` →
  `[(0,90),(120,150)]`（第二处空隙正好 30，不合并）。
- `overlaps(a, b)`：闭区间是否真正交叠，**仅端点相接不算交叠**，判据
  `a[0] < b[1] and b[0] < a[1]`。额外自测 `overlaps((0,10),(2,3)) is True`。

### 5. `units/intervals/intervals.py` — `Impl.merge` / `subtract` / `total_length`
- `merge(ranges, gap=0)`：按 `lo` 排序，相邻两段缺失整数数
  `next.lo - prev.hi - 1 <= gap` 时合并（含嵌套/逆序输入）；返回升序元组列表，空输入 `[]`。
- `subtract(ranges, hole)`：**先对输入做 `merge`（gap=0）归一化**，再逐段挖去闭区间
  `hole`：不相交原样保留；完全覆盖则丢弃；横跨则在两侧生成 `(lo, hlo-1)` / `(hhi+1, hi)`；
  结果升序。
- `total_length(ranges)`：`merge` 后对每段 `hi - lo + 1` 求和（重叠只算一次），空输入 0。

### 6. `units/flags/flags.py` — `Impl.parse(argv)`
返回 `{"values": {...}, "flags": {...}, "positional": [...]}`：
- 不以 `-` 开头、或正好是 `-` 的项按顺序进 `positional`。
- `--` 之后全部进 `positional`，`--` 本身不出现。
- `--key=value` / `--key value` 记入 `values`（值为列表，按出现顺序累积）。
- `--key` 后面没有可用值（已到结尾，或下一项以 `--` 开头）时为开关，记入 `flags`；
  **注意单个 `-` 开头的项（如 `-5`）可以作为值**，只有 `--` 前缀会阻断取值。
- `-abc` 解析为开关 `a`、`b`、`c`；开关重复仍为 `True`。
- 同一 key 可同时出现在 `values` 与 `flags`（如 `--x y --x`）。

### 7. `units/measure/measure.py` — `Impl.to_cm(value, unit)`
- `m → ×100`，`cm → ×1`，`mm → ×0.1`（`10 mm = 1 cm`）。
- 未知单位抛 `ValueError`（`ft` 等）。整数值返回 `int`（如 `1.5 m → 150`）。

### 8. `units/slugify/slugify.py` — `Impl.slugify(text)`
- 小写化 → 把连续非 `[a-z0-9]` 折叠成单个 `-` → 去掉首尾 `-`；
  结果为空时返回 `"untitled"`。

### 9. `units/ranges/ranges.py` — `Impl.merge(ranges)`
- 闭区间、按 `lo` 排序合并有交叠（`lo <= prev.hi`）的段，取较大 `hi`；
  **仅相邻（无重叠）不合并**（如 `(1,4)` 与 `(5,7)` 保持分开）；返回升序元组列表，空输入 `[]`。

### 10. `units/alpha/alpha.py` — `add`
- 修复为 `a + b`（原为 `a - b`）；`check.py` 断言 `add(2,3)==5`。

### 11. `units/beta/beta.py` — `mul`
- 修复为 `a * b`（原为 `a + b`）；`check.py` 断言 `mul(3,4)==12`。

### 12. `units/chain/` — `stage1`…`stage4`
- `stage1.parse_orders(path)`：读取表头 `id,qty,price` 的 CSV，返回
  `[{"id":int,"qty":int,"price":int}, ...]`。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行。
- `stage3.total_by_price(rows)`：按 `price` 汇总 `qty`，返回 `{price: total_qty}`。
- `stage4.report(totals)`：按 `price` 升序，每行 `"{price}:{qty}\n"` 拼接；
  例如 `{10:2,7:4}` → `"7:4\n10:2\n"`。

## 未改动的冻结文件（sha256）

```
588db2b78e9d1365dd8d7563abe70812bf958955665a95ad02b1e6a16c793724  units/csvfix/test_csvfix.py
4c425638c58c34872b0800f5fe19092c011785e123c15a1c69e2e99d762a5af8  units/rules/test_rules.py
249141060049bcdd4c266e0448a139d5f8367d3969491c005ccdb5e4082747c8  units/ledger/test_ledger.py
d0eac968ccaa16dd71ca63f9a264f9aca424a03de2e94c6d77e607d7a5585b14  units/schedule/test_schedule.py
3c5fa1bb8a23b1d96acd8df686bce8cd73eb428e6ca6548279f8c3a1b96e0580  units/intervals/test_intervals.py
07edf2bc2445551aa24d467a4ed09baca7cff1b8e76aa0760453440b0e521960  units/flags/test_flags.py
4d1ca7a8eb9de778485e1a098269aebae5763096bcf87f2b9a9480a5268d766d  units/measure/test_measure.py
fc56ef2fd6b0007cca58e323e841dc6b75be168d1a08ead13905ace1d134c9fd  units/slugify/test_slugify.py
4efde602ab3325b7e35c7c59b29a48739f705fa33a335c8eb4d2f11d82507a62  units/ranges/test_ranges.py
8f30e265cbfa6ae5fdcf42d313033cd960d83bd1d7cd7f8f7ba3ca09eb289dd4  units/alpha/check.py
4e7ac3aa4b4ddcc08f0e4902f7e25de1af457b7cbcce051aad1394b80ca373e1  units/beta/check.py
13aa797e9aef3c9aba1c20677f6fec223964e6c285658c224dae79e3efdc3670  units/chain/tests/test_stage1.py
b48a335be723c00645c2305619266096635a4c2de66703e2987af54a4ece56e5  units/chain/tests/test_stage2.py
ed5c68b1370162f168d99306dc6cd7feca0d8668757ed0acb7d97a57b130ad32  units/chain/tests/test_stage3.py
c3d2971eca0f4e523fd9a27a4d017ccc7797dbaf8b8abeffdd411c300fe8c453  units/chain/tests/test_stage4.py
```

## 实际改动的实现文件

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
- `units/chain/stage1.py`, `stage2.py`, `stage3.py`, `stage4.py`

（运行 pytest 生成的 `units/*/.pytest_cache/` 为副产物，不含源码改动。）
