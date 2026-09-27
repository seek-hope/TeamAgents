# 12 块独立交付物 — 实现与验证报告

日期：2026-09-24 ・ Python 3.13.15 ・ 工作目录：本仓库根目录（`twelve-deliverables.B.2/`）

规则遵守：**没有修改任何测试 / 验收文件**（`test_*.py`、`check.py`、`tests/` 全部原样）。
所有结果均来自下面列出的真实命令。

---

## 1. `units/csvfix/impl.py` — `parse` / `total`

**改动文件**：`units/csvfix/impl.py`

语义边界：

- `parse(line)`：一行固定 3 列 `[名称, 数量, 单价]`。
  - 支持双引号包裹字段：引号内的逗号不拆分，`""` 表示一个字面引号，引号本身不作为数据保留。
  - 每个字段去除首尾空白。
  - 列数不等于 3、引号未闭合、非字符串输入 → 返回 `None`。
- `total(rows)`：每行金额 = `数量 * 单价`（**第三列本身不是金额**，这正是原 docstring 指出的 bug）。
  - 数量或单价为空白（`""` / 纯空白）的行被**跳过**，不当作 0、也不计入合计。
  - 非空但无法转成整数的字段 → 抛 `ValueError`（报错而非静默当 0）。
  - `len(row) != 3` → 抛 `ValueError`。

> 说明：可见测试 `total([["a","1","2"],["b","2",""]]) == 2` 对“按第三列求和”和“数量×单价”两种读法结果都是 2；
> 由于 docstring 明确把“把第三列当成金额”标为 bug，这里取 `数量 × 单价` 的语义，并在结果为空/非法时给出上述边界。

验证命令与结果：

```
$ cd units/csvfix && python3 -m pytest -q
2 passed in 0.02s
```

额外探针（真实运行）：`parse('"a,b" , 2 , x') -> ['a,b','2','x']`；`parse('a,2') -> None`；`parse('"he said ""hi""" , 1 , 2') -> ['he said "hi"','1','2']`；`total([["a","1","oops"]])` 抛 `ValueError`。

---

## 2. `units/rules/engine.py` — `evaluate`

**改动文件**：`units/rules/engine.py`

语义边界：

- `{"all": [...]}`：全部事实名为真才为真；空列表为真（空真）。
- `{"any": [...]}`：任一为真即为真；空列表为假。
- 事实由 `facts` 按名查取，缺失的名字视为假；返回值是真正的 `bool`。
- 同时含 `all` 与 `any` 时按 `all`；两者都没有或 `rules` 非字典 → `False`。

验证：`cd units/rules && python3 -m pytest -q` → **1 passed**。

---

## 3. `units/ledger/ledger.py` — `Ledger.apply` / `balance`

**改动文件**：`units/ledger/ledger.py`

语义边界：

- `apply(rows, op)`：`rows = [(kind, account, cents), ...]`，返回**属于 `op` 账户**的行，保持原顺序。
  - `kind` 只允许 `"deposit"` / `"withdraw"`，其他值抛 `ValueError`。
  - 金额为负抛 `ValueError`；整批输入都会校验（不只是目标账户的行）。
- `balance(rows, account)`：先按账户过滤，`deposit` 加、`withdraw` 减，无该账户行 → `0`。
  - 其他 `kind` 抛 `ValueError`（不静默吞掉金额）。

验证：`cd units/ledger && python3 -m pytest -q` → **2 passed**。
额外探针：`apply([...], "a")` 返回顺序正确的 2 行，`balance(...,"a")==70`，`balance(...,"b")==0`，负金额抛 `ValueError`。

---

## 4. `units/schedule/schedule.py` — `slots` / `overlaps`

**改动文件**：`units/schedule/schedule.py`

语义边界：

- `slots(ranges, minutes)`：先按起点升序排序（端点反了的区间归一化），
  空隙 `next_lo - prev_hi` **严格小于** `minutes` 才合并（等于时不合并）；
  返回按起点升序的 `(lo, hi)` 元组列表；空输入 → `[]`。
- `overlaps(a, b)`：半开式判断 `a_lo < b_hi and b_lo < a_hi`，端点相接不算交叠，返回 `bool`。

验证：`cd units/schedule && python3 -m pytest -q` → **2 passed**。
额外探针：`slots([(0,60),(30,90),(120,150)], 30) == [(0,90),(120,150)]`；`overlaps((0,10),(10,20)) is False`。

---

## 5. `units/intervals/intervals.py` — `Impl.merge` / `subtract` / `total_length`

**改动文件**：`units/intervals/intervals.py`

语义边界：

- `merge(ranges, gap=0)`：按 lo 升序；相邻段**缺失的整数个数** `next.lo - prev.hi - 1 <= gap` 就合并
  （负数即重叠/包含，一定合并）。默认 `gap=0` 时“数值相邻”也合并。返回元组列表，空输入 → `[]`。
- `subtract(ranges, hole)`：先用 `merge` 归一化输入；与 hole 不相交的段原样保留，被完全覆盖的段消失，
  横跨 hole 的段按端点分裂（端点相接也算相交）。
- `total_length(ranges)`：先 `merge`（重叠/相邻只算一次），再累加每段 `hi - lo + 1`。

验证：`cd units/intervals && python3 -m pytest -q` → **7 passed**。
额外探针：`merge([[1,2],[4,5]], 1) == [(1,5)]`；`subtract([[5,7],[1,3]], (2,6)) == [(1,1),(7,7)]`；`total_length([[0,2],[2,4]]) == 5`。

---

## 6. `units/flags/flags.py` — `Impl.parse`

**改动文件**：`units/flags/flags.py`

语义边界：

- 非 `"-"` 开头、或正好 `"-"` 的项按顺序进 `positional`。
- `--` 之后所有项都是位置参数，`--` 自身不出现。
- `--key=value` 与 `--key value` 都是键值对；`--key=` 允许空字符串值。
- `--key` 后面无可用值（到结尾，或下一项以 `--` 开头）时是开关。
- 单横线 `-abc` 一律拆成开关 `a,b,c`（短选项不吞值），所以 `-5` 只有当“值”出现在 `--k` 后面时才作值。
- `values` 的值是列表、按键出现顺序累积；开关重复仍是 `True`；同一个键可以既有值又当开关。

验证：`cd units/flags && python3 -m pytest -q` → **8 passed**。
额外探针：`parse(["--k=v","-ab","x","--","--y","z"]) == {'values':{'k':['v']}, 'flags':{'a':True,'b':True}, 'positional':['x','--y','z']}`；`parse(["--k","-5"])['values'] == {'k':['-5']}`。

---

## 7. `units/measure/measure.py` — `Impl.to_cm`

**改动文件**：`units/measure/measure.py`

语义边界：只支持 `"m"`（×100）、`"cm"`（×1）、`"mm"`（÷10）；其他任何单位（含 `None`、`"M"`）抛 `ValueError`。返回值是数值（米/毫米可能是浮点）。

验证：`cd units/measure && python3 -m pytest -q` → **1 passed**。
额外探针：`to_cm(1.5,"m")==150`、`to_cm(20,"mm")==2`；`"ft"`/`None`/`"M"` 均抛 `ValueError`。

---

## 8. `units/slugify/slugify.py` — `Impl.slugify`

**改动文件**：`units/slugify/slugify.py`

语义边界：整体小写化 → 把所有非 `a-z0-9` 的连续片段折叠成一个 `-` → 去掉首尾 `-` → 结果为空返回 `"untitled"`。

验证：`cd units/slugify && python3 -m pytest -q` → **1 passed**。
额外探针：`slugify("Hello, World!")=="hello-world"`、`slugify("  --A@@B-- ")=="a-b"`、`slugify("***")=="untitled"`。

---

## 9. `units/ranges/ranges.py` — `Impl.merge`

**改动文件**：`units/ranges/ranges.py`

语义边界：每段归一化为 `(min,max)` 后按起点升序；`next.lo <= prev.hi`（重叠**或端点相接**）才合并；
仅数值相邻但不接触（如 `[1,4]` 与 `[5,7]`）**不**合并。空输入 → `[]`，返回元组列表。

验证：`cd units/ranges && python3 -m pytest -q` → **1 passed**。
额外探针：`merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]`；`merge([[1,4],[5,7]]) == [(1,4),(5,7)]`（相邻不合并）。

---

## 10. `units/alpha/alpha.py`

**改动文件**：`units/alpha/alpha.py`（`add` 由 `a - b` 改为 `a + b`；未改 `check.py`）

验证：`cd units/alpha && python3 check.py` → 输出 `alpha ok`。

---

## 11. `units/beta/beta.py`

**改动文件**：`units/beta/beta.py`（`mul` 由 `a + b` 改为 `a * b`；未改 `check.py`）

验证：`cd units/beta && python3 check.py` → 输出 `beta ok`。

---

## 12. `units/chain/` — `stage1`…`stage4`

**改动文件**：`units/chain/stage1.py`、`stage2.py`、`stage3.py`、`stage4.py`

语义边界：

- `stage1.parse_orders(path)`：`csv.DictReader` 读取表头 `id,qty,price`，三列转 `int`，保持文件顺序，
  返回 `[{"id":int,"qty":int,"price":int}, ...]`。
- `stage2.filter_orders(rows)`：只保留 `qty > 0`，保持原顺序，返回新列表。
- `stage3.total_by_price(rows)`：按 `price` 汇总 `qty`，返回 `{price: 总数量}`。
- `stage4.report(totals)`：按价格数值升序输出 `"{price}:{qty}\n"`，空字典 → `""`。

验证：`cd units/chain && python3 -m pytest -q tests/` → **4 passed**。
额外端到端探针（真实运行）：

```
filter_orders(parse_orders("units/chain/data/orders.csv"))
  -> [{'id':1,'qty':2,'price':10},{'id':3,'qty':3,'price':7},{'id':4,'qty':1,'price':7}]
total_by_price(...) -> {10: 2, 7: 4}
report(...) -> '7:4\n10:2\n'
```

---

## 汇总验证命令（全部真实运行，工作目录为本包根目录）

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
| 10 | alpha | `cd units/alpha && python3 check.py` | `alpha ok` |
| 11 | beta | `cd units/beta && python3 check.py` | `beta ok` |
| 12 | chain | `cd units/chain && python3 -m pytest -q tests/` | 4 passed |

**12/12 全部通过**；所有测试/验收文件保持原样未改。
