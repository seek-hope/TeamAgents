# REPORT — 六块独立交付物

本报告逐块说明实现后的语义边界（尤其边界情况），以及**真实运行过**的命令与结果。
所有测试文件均未修改。

环境：Python 3.13.15，pytest 9.0.3，工作目录
`.../work/six-deliverables.B.1`。

---

## 1. `units/csvfix/impl.py` — `parse` / `total`

**语义边界**

- `parse(line)`：
  - 逗号分隔，**恰好 3 列**；列数不为 3（含空行、多列、少列）返回 `None`。
  - 支持双引号字段：`"a,b"` 中的逗号属于字段内容；字段内 `""` 表示一个字面量引号；
    未闭合引号按已解析内容处理。
  - 对每个字段裁剪首尾空白，所以 `' a , 2 , x '` → `["a", "2", "x"]`。
- `total(rows)`：
  - 金额列取**第 3 列（索引 2）**。
  - 空白金额跳过（不累加、不报错）。
  - 非整数金额抛 `ValueError`（对应测试名里的 “reports bad”）。
  - 列数不足 3 的行整体跳过。
  - 注：原始 docstring 对“第三列是否为金额”表述含糊，但冻结的测试期望
    `total([["a","1","2"],["b","2",""]]]) == 2`，唯有按第 3 列求和才成立，故以测试为准。

**验证命令与结果**

```
cd units/csvfix && python3 -m pytest -q
# 2 passed in 0.00s
```

补充手工边界检查（真实执行）：
`parse('"a,b", 2 , x') == ['a,b', '2', 'x']`；`parse('a,2,3,4') is None`；
`total([['a','1',''],['b','2','3']]) == 3`；`total([['a','1','zz']])` 抛 `ValueError`。

---

## 2. `units/rules/engine.py` — `evaluate`

**语义边界**

- `{"all": [...]}`：所有 fact 均为真才为真；空列表为 `True`（空合取）。
- `{"any": [...]}`：任一 fact 为真即为真；空列表为 `False`（空析取）。
- 缺失的 fact 视为 `False`（用 `facts.get(key, False)`）。
- 返回值恒为真正的 `bool`（测试用 `is True` / `is False`）。
- 同时给出 `all` 与 `any` 时两者都需满足；两个键都没有时返回 `False`。

**验证命令与结果**

```
cd units/rules && python3 -m pytest -q
# 1 passed in 0.00s
```

---

## 3. `units/ledger/ledger.py` — `Ledger.apply` / `balance`

**语义边界**

- 行格式 `(kind, account, cents)`，`kind ∈ {"deposit", "withdraw"}`。
- `apply(rows, account)`：先对**全部行**做校验（未知 `kind` 抛 `ValueError`；`cents < 0`
  抛 `ValueError`），再返回属于该账户的行，保持原顺序。负数校验作用于全部行，而不只是
  目标账户的行。
- `balance(rows, account)`：只统计该账户，deposit 相加、withdraw 相减；该账户没有行返回 `0`。
- 参数名沿用了文档里的 `op`，实际语义是账户名（测试以 `"a"` 调用）。

**验证命令与结果**

```
cd units/ledger && python3 -m pytest -q
# 2 passed in 0.00s
```

补充手工检查：非目标账户的负数行同样抛 `ValueError`。

---

## 4. `units/schedule/schedule.py` — `slots` / `overlaps`

**语义边界**

- `slots(ranges, minutes)`：闭区间合并。排序后相邻两段空隙 `next_lo - prev_hi`
  **严格小于** `minutes` 才合并，等于 `minutes` 时不合并（测试里 `(0,90)` 与 `(120,150)`
  空隙恰为 30 保持分开）。嵌套区间被吸收。结果按起点升序、返回元组列表，空输入 `[]`。
  `minutes <= 0` 时只有重叠/包含才合并（端点相接不合并）。
- `overlaps(a, b)`：闭区间，仅当有正长度交叠时为 `True`；端点恰好相接
  `(0,10)` 与 `(10,20)` 返回 `False`；退化为单点相接也返回 `False`。
  实现为 `a_lo < b_hi and b_lo < a_hi`。

**验证命令与结果**

```
cd units/schedule && python3 -m pytest -q
# 2 passed in 0.00s
```

补充手工检查：`slots([(0,10),(20,30)],10)` 保持分开；`slots([(0,100),(10,20)],0)` 合并为
`[(0,100)]`；`overlaps((5,5),(5,5)) is False`。

---

## 5. `units/intervals/intervals.py` — `merge` / `subtract` / `total_length`

**语义边界**

- `merge(ranges, gap=0)`：按 `lo` 升序排序；相邻两段间**缺失的整数个数**
  `next.lo - prev.hi - 1 <= gap` 就合并（默认 `gap=0`，即端点相接/重叠即合并）。
  嵌套与逆序段被正常处理（如 `[[0,10],[2,3]]` → `[(0,10)]`）。空输入 `[]`。返回元组列表。
- `subtract(ranges, hole)`：先以 `merge`（gap=0）归一化输入，再挖去闭区间 `hole`。
  不相交的段原样保留，完全覆盖的段消失，横跨 hole 的段分裂为两段
  （`hi == hole_lo` / `lo == hole_hi` 的“相切”只保留剩余部分）。
- `total_length(ranges)`：先归一化去重，再求和 `hi - lo + 1`（闭区间含端点，重叠只算一次）。

**验证命令与结果**

```
cd units/intervals && python3 -m pytest -q
# 7 passed in 0.01s
```

补充手工检查：`subtract([[0,4],[8,9]],(5,7)) == [(0,4),(8,9)]`；
`subtract([[5,7],[1,3]],(2,6)) == [(1,1),(7,7)]`；`total_length([[0,2],[2,4]]) == 5`。

---

## 6. `units/flags/flags.py` — `Impl.parse`

**语义边界**

- 不以 `-` 开头、或正好是 `-` 的项按出现顺序进入 `positional`。
- 单独的 `--` 是终止符：其后所有项（包括像选项的字符串）都进 `positional`，`--` 自身不出现。
- `--key=value` 与 `--key value` 均为键值对；`--key` 无可用的下一个值
  （到结尾，或下一项以 `--` 开头）时是开关。
- 下一项只要**不以 `--` 开头**就可作为值（因此 `--k -5` 取到 `"-5"`）。
- `-abc` 展开为开关 `a`、`b`、`c`。
- `values` 的值是列表，重复键按出现顺序累积；开关重复出现仍是 `True`。

**验证命令与结果**

```
cd units/flags && python3 -m pytest -q
# 8 passed in 0.00s
```

补充手工检查：`parse(['--k','-5'])['values'] == {'k': ['-5']}`；
`parse(['--','--x','y'])['positional'] == ['--x','y']` 且 `flags == {}`；
`parse(['--k','1','--k','2'])['values'] == {'k': ['1','2']}`。

---

## 汇总

从仓库根目录一次性收集全部 6 个测试文件也通过：

```
python3 -m pytest -q
# 22 passed in 0.02s
```

| 单元 | 命令 | 结果 |
| --- | --- | --- |
| csvfix | `cd units/csvfix && python3 -m pytest -q` | 2 passed |
| rules | `cd units/rules && python3 -m pytest -q` | 1 passed |
| ledger | `cd units/ledger && python3 -m pytest -q` | 2 passed |
| schedule | `cd units/schedule && python3 -m pytest -q` | 2 passed |
| intervals | `cd units/intervals && python3 -m pytest -q` | 7 passed |
| flags | `cd units/flags && python3 -m pytest -q` | 8 passed |
| 全部 | `python3 -m pytest -q` | 22 passed |

**改动文件**：`units/csvfix/impl.py`、`units/rules/engine.py`、`units/ledger/ledger.py`、
`units/schedule/schedule.py`、`units/intervals/intervals.py`、`units/flags/flags.py`、
`REPORT.md`。测试文件未改动。
