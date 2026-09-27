# 六块交付物实现报告

工作区包含 6 个互相独立的单元，各自有已冻结的验收测试。本报告逐块说明实现的语义边界
（尤其是边界情况）以及**真实运行过**的命令与结果。所有测试文件均未修改。

## 总览：真实运行过的命令与结果

命令（逐目录运行，`exit=$?` 为 pytest 真实退出码）：

```
for u in csvfix rules ledger schedule intervals flags; do (cd units/$u && python3 -m pytest -q >/dev/null 2>&1); echo "$u exit=$?"; done
```

真实结果：

| 单元 | 命令 | 结果 | 退出码 |
| --- | --- | --- | --- |
| csvfix | `cd units/csvfix && python3 -m pytest -q` | `2 passed in 0.02s` | 0 |
| rules | `cd units/rules && python3 -m pytest -q` | `1 passed in 0.02s` | 0 |
| ledger | `cd units/ledger && python3 -m pytest -q` | `2 passed in 0.02s` | 0 |
| schedule | `cd units/schedule && python3 -m pytest -q` | `2 passed in 0.02s` | 0 |
| intervals | `cd units/intervals && python3 -m pytest -q` | `7 passed in 0.04s` | 0 |
| flags | `cd units/flags && python3 -m pytest -q` | `8 passed in 0.04s` | 0 |

另外用一段临时 Python 脚本（在 `units/` 下，未写入任何文件）直接调用各实现，
回答了边界样例，输出见下文每节的「额外验证」。

修改的文件（仅实现文件，测试文件未动）：

- `units/csvfix/impl.py`
- `units/rules/engine.py`
- `units/ledger/ledger.py`
- `units/schedule/schedule.py`
- `units/intervals/intervals.py`
- `units/flags/flags.py`

---

## 1. csvfix — `parse` / `total`

**语义**：固定 3 列的 CSV 行。

- `parse(line)`：用标准库 `csv.reader` 做引号处理，然后对每个字段 `strip()`。
  引号内的逗号不分割字段；结果只保留去掉两端空白后的原始字符串（不做数值转换）。
- 边界：
  - 列数不是 3 → 返回 `None`（`'a,2'` → `None`；`'a,b,c,d'` → `None`）。
  - `csv.Error`（引号不闭合等）→ 返回 `None`。
  - 带引号字段：`'"a,b", 2, x'` → `['a,b', '2', 'x']`。
  - 空白裁剪：`' a , 2 , x '` → `['a', '2', 'x']`。
- `total(rows)`：以**第 3 列**为金额累加。行少于 3 列抛 `ValueError`；
  第 3 列为空白（`strip()` 后为空）**跳过**（等价贡献 0，不报错）；
  第 3 列非数字抛 `ValueError`。
  - 测试数据 `[["a","1","2"],["b","2",""]]` → `2`（第三列 2 + 跳过空）。

额外验证输出：
```
csv quoted: ['a,b', '2', 'x']
csv 4col: None
csv 2col: None
csv bad -> ValueError
csv blank: 3        # [["a","1",""],["b","2","3"]] -> 跳过空后 0+3
```

## 2. rules — `engine.evaluate(rules, facts)`

**语义**：单键规则字典。

- `{"all": [...]}` → 所有键为真才 `True`；空列表 `all([])` 为 `True`（空真）。
- `{"any": [...]}` → 任一键为真即 `True`；空列表为 `False`。
- 未出现在 `facts` 中的键视为 `False`（用 `facts.get(k, False)`），返回值经 `bool()` 归一为 `True`/`False`。
- 边界：`rules` 不是「恰好一个键的 dict」、或键不是 `all`/`any` → 抛 `ValueError`。

额外验证输出：
```
all []: True any []: False
missing key: False
rules unknown -> ValueError
```

## 3. ledger — `Ledger.apply` / `balance`

**语义**：只看单个账户的行列表 `[(kind, account, cents), ...]`。

- `apply(rows, op)`：
  - 先校验**输入中的每一行**（包括其他账户的行）：`kind` 必须是 `deposit`/`withdraw`，
    否则抛 `ValueError`；`cents < 0` 抛 `ValueError`。
  - 然后返回 `account == op` 的行，**保持原顺序**，不修改输入。
  - 边界：没有匹配行 → `[]`；空输入 → `[]`（不抛错）。
- `balance(rows, account)`：`deposit` 加、`withdraw` 减，返回净额整数；
  没有该账户的行 → `0`；空列表 → `0`；遇到未知 `kind` 抛 `ValueError`。

额外验证输出：
```
empty acct: [] 0
ledger neg(other acct) -> ValueError    # 负金额即使属于别的账户也拒绝
ledger badkind -> ValueError
```

## 4. schedule — `slots` / `overlaps`

**语义**：闭区间时间段。

- `slots(ranges, minutes)`：按起点升序排序后线性合并。相邻两段 `prev=(pl,ph)`、
  `cur=(cl,ch)` 满足 `cl - ph < minutes` 即合并为 `(pl, max(ph, ch))`；否则分段。
  - 边界：空输入 → `[]`；单段原样返回；被包含的段（nested）并入外层并取更远的右端点；
    空隙恰好等于 `minutes` 不合并（`(0,60),(120,150)` 空隙 30、`minutes=30` → 两段）。
  - 返回值是按起点升序的 `(lo, hi)` 元组列表。
- `overlaps(a, b)`：闭区间交叠判定为 `a.lo < b.hi and b.lo < a.hi`。
  - 边界：端点相接 `(0,10)` 与 `(10,20)` → `False`；退化为点且落在另一区间内 `(5,5)` 与 `(0,10)` → `True`。

额外验证输出：
```
sch empty: []
sch merge nested: [(0, 100)]
sch touch thr: [(0, 10), (40, 50)]      # 空隙恰好 30 不合并
ovl touch: False ovl pt: True
```

## 5. intervals — `Impl.merge` / `subtract` / `total_length`

**语义**：闭整数区间，端点均为整数，`lo <= hi`。

- `merge(ranges, gap=0)`：按 `lo` 升序排序；相邻两段之间**缺失的整数个数**
  `missing = next.lo - prev.hi - 1`，`missing <= gap` 就合并，右端点取 `max`。
  - 边界：空输入 → `[]`；相邻（`missing == 0`）合并；nested 段并入外层；
    `gap` 可以是任意非负整数，`[[10,12],[0,1],[5,6]]` 在 `gap=3` 时全部并为 `(0,12)`。
  - 返回升序 `(lo, hi)` 元组列表。
- `subtract(ranges, hole)`：先把 `ranges` 用 `merge(gap=0)` 规范化，再逐段挖 `hole`。
  - 与 `hole` 不相交（`hi < hole.lo` 或 `lo > hole.hi`）→ 原样保留；
  - 左侧剩余 `(lo, hole.lo-1)` 若 `lo <= hole.lo-1`（非空）；右侧剩余
    `(hole.hi+1, hi)` 若 `hi >= hole.hi+1`；被完全覆盖则两段都不产生。
  - 边界：`subtract([[0,10]], (0,10))` → `[]`；`subtract([[0,4]], (4,9))` → `[(0,3)]`；
    无序输入先规范化（测试 `test_subtract_normalizes_its_input_first`）。
- `total_length(ranges)`：先 `merge()` 再对每段累加 `hi - lo + 1`；重叠只算一次；空输入 → `0`。

额外验证输出：
```
iv merge default join: [(0, 1)]
iv subtract disjoint: [(0, 1), (10, 11)]
iv total overlap: 21
iv total empty: 0
```

## 6. flags — `Impl.parse(argv)`

**语义**：返回 `{"values": {...}, "flags": {...}, "positional": [...]}`。

- 分类规则：
  - 不以 `-` 开头、或恰好等于 `-` 的项 → 按出现顺序进入 `positional`。
  - `--` → 置「选项结束」标志，`--` 自身不出现；其后所有项（含形如 `--x`、`-x` 的）
    全部进 `positional`。
  - `--key=value` → `values["key"].append("value")`（允许空值 `--k=`）。
  - `--key value`：若下一项存在且**不以 `--` 开头**，则作为值消费（因此 `--k -5` 的值是 `-5`）；
    否则本项是开关 `flags["key"] = True`（后面到结尾，或紧跟另一个 `--` 开头的项）。
  - `-abc` → 三个开关 `a`、`b`、`c`；`-key=value`（短选项含 `=`）→ 视作键 `key`、值 `value`。
  - `values` 的值一律是**列表**，重复键按出现顺序累积；开关重复出现仍为 `True`；
    同名键可同时出现在 `values` 与 `flags` 中（测试 `--x y --x`）。
  - 未覆盖的边界（文档字符串未规定）：`-abc` 形式的短选项一律视为纯开关，不会去消费下一项的值；
    仅含 `=` 的短选项按键值对处理。
- 边界：空输入 → `{"values": {}, "flags": {}, "positional": []}`。

额外验证输出：
```
flags dashdash: {'values': {}, 'flags': {}, 'positional': ['-x', '--y']}
flags k=v: {'values': {'k': ['', 'v']}, 'flags': {}, 'positional': []}
flags short value-ish: {'values': {'k': ['-abc']}, 'flags': {}, 'positional': []}
flags empty: {'values': {}, 'flags': {}, 'positional': []}
```

---

## 说明

- 六块相互独立，未引入任何跨单元依赖；未新增依赖，仅使用标准库。
- 验收测试文件（`units/*/test_*.py`）在整个过程中未被修改。
