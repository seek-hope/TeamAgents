# 12 个独立交付物 —— 实现报告

本报告由负责集成的 leader 汇总，所有命令均在本工作区真实执行过；
12 个单元互不依赖，各自有一名 worker 实现，leader 最后独立复跑了全部验收命令。

约束遵守情况：

- 未修改任何测试 / 验收文件（`units/*/test_*.py`、`units/chain/tests/*.py`、`units/*/check.py`）。
  这些文件的 mtime 仍为仓库原有时间（2026-09-24 / 2026-09-28），内容与初始快照一致。
- 每个单元只改动了其实现文件，未跨单元改动。

---

## 1. `units/csvfix/` — `impl.parse` / `impl.total`

改动文件：`units/csvfix/impl.py`

语义边界：

- `parse(line)`：用 `csv.reader` 解析一行，支持引号包裹（字段内逗号算一个字段）；对每个字段
  `strip()` 去空白；字段数不等于 3（`COLUMN_COUNT`）时返回 `None`；`line is None` 返回 `None`。
- `total(rows)`：只累加第 3 列（`AMOUNT_COLUMN=2`）；空串 / 纯空白字段跳过；短于 3 列或 `None`
  行跳过；`None` 字段跳过。
- 边界示例（实测）：`parse('"a,b", 2 , 3 ') == ['a,b','2','3']`；`parse('') is None`；
  `total([['a','1','2'],['b','2',''],['c','9']]) == 2`。

真实运行命令与结果：

```
cd units/csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s
EXIT=0
```

## 2. `units/rules/` — `engine.evaluate(rules, facts)`

改动文件：`units/rules/engine.py`

语义边界：

- `{"all": [...]}`：所有名字对应 fact 为真才为 `True`（空列表为 `True`，空真）。
- `{"any": [...]}`：至少一个为真（空列表为 `False`）。
- 缺失 fact 视为 `False`（`facts.get(name, False)`）。
- 同时含 `all` 与 `any` 的映射，两者都要成立（逻辑与）。
- 规则不含 `all` / `any` 抛 `ValueError`；`rules` 不是 dict 抛 `TypeError`。

真实运行命令与结果：

```
cd units/rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
EXIT=0
```

## 3. `units/ledger/` — `Ledger.apply` / `balance`

改动文件：`units/ledger/ledger.py`

语义边界：

- `Ledger.apply(rows, op)`：先扫描全部行，任一金额为负立即抛 `ValueError`；随后返回
  **account == op** 的行，保持原有顺序（`op` 是要筛的账户名）。
- `balance(rows, account)`：`deposit` 加、`withdraw` 减；无该账户行返回 `0`；未知 kind 抛
  `ValueError`。
- 说明：`apply` 只校验金额非负，不校验 kind（docstring 只要求对负金额报错）。

真实运行命令与结果：

```
cd units/ledger && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s
EXIT=0
```

## 4. `units/schedule/` — `slots` / `overlaps`

改动文件：`units/schedule/schedule.py`

语义边界（闭区间）：

- `slots(ranges, minutes)`：先按 `lo` 排序，若 `next_lo - prev_hi < minutes` 则并入当前段
  （`hi` 取较大者）；否则另起一段；返回按起点升序的列表；空输入 `[]`。
- `overlaps(a, b)`：严格不等式 `a_lo < b_hi and b_lo < a_hi`，端点相接
  （如 `(0,10)` 与 `(10,20)`）返回 `False`；返回真正的 `bool`。

真实运行命令与结果：

```
cd units/schedule && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s
EXIT=0
```

## 5. `units/intervals/` — `Impl.merge` / `subtract` / `total_length`

改动文件：`units/intervals/intervals.py`

语义边界（整数闭区间）：

- `merge(ranges, gap=0)`：按 `lo` 排序，若相邻两段缺失整数个数 `next_lo - prev_hi - 1 <= gap`
  则合并，`hi` 取较大者（因此嵌套 / 乱序输入也能正确折叠）；返回升序元组列表；空输入 `[]`。
- `subtract(ranges, hole)`：先用 `merge(ranges)` 归一化；与 hole 不相交的段原样保留，被完全
  覆盖的段消失，横跨 hole 的段分裂为 `(lo, hole_lo-1)` 与 `(hole_hi+1, hi)`。
- `total_length(ranges)`：对 `merge` 后的并集求 `Σ(hi-lo+1)`，重叠整数只计一次。

真实运行命令与结果：

```
cd units/intervals && python3 -m pytest -q
.......                                                                  [100%]
7 passed in 0.00s
EXIT=0
```

## 6. `units/flags/` — `Impl.parse`

改动文件：`units/flags/flags.py`

语义边界（单遍扫描 argv）：

- 非 `-` 开头、或恰好为 `-` 的项按出现顺序进 `positional`。
- `--` 之后所有项进 `positional`，`--` 自身不出现。
- `--key=value` 与 `--key value` 都进 `values[key]`（值为列表，按出现顺序累积）。
- `--key` 后面没有可用值（到结尾，或下一项以 `--` 开头）→ `flags[key]=True`。
- `-abc` 展开为开关 `a`/`b`/`c`；重复开关仍为 `True`。
- 单个 `-` 开头的非 `--` 项（如 `-5`）可作为取值。

真实运行命令与结果：

```
cd units/flags && python3 -m pytest -q
........                                                                 [100%]
8 passed in 0.00s
EXIT=0
```

## 7. `units/measure/` — `Impl.to_cm(value, unit)`

改动文件：`units/measure/measure.py`

语义边界：

- 换算因子：`m` → ×100，`cm` → ×1，`mm` → ×0.1。
- 非法 / 不可哈希 / `None` 单位抛 `ValueError`。
- 返回值为数值（`1.5 m → 150.0`，与 `150` 相等比较成立）。

真实运行命令与结果：

```
cd units/measure && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
EXIT=0
```

## 8. `units/slugify/` — `Impl.slugify(text)`

改动文件：`units/slugify/slugify.py`

语义边界：

- 逐字符处理：字母数字（`str.isalnum`，Unicode 感知）小写保留，其余字符变 `-`。
- `-+` 折叠为单个 `-`；去掉首尾 `-`；结果为空则返回 `"untitled"`。
- 边界示例：`"  --A@@B-- " → "a-b"`，`"***" → "untitled"`。

真实运行命令与结果：

```
cd units/slugify && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
EXIT=0
```

## 9. `units/ranges/` — `Impl.merge(ranges)`

改动文件：`units/ranges/ranges.py`

语义边界（闭区间，允许相接）：

- 按起点排序后扫描；`next_start <= current_end` 即视为重叠或相接并合并，`end` 取较大者。
- 返回按起点排序的 `(start, end)` 元组列表；空 / 假值输入返回 `[]`。
- 边界示例：`[[5,7],[1,3],[2,4]] → [(1,4),(5,7)]`；`[[1,2],[2,3]] → [(1,3)]`（相接合并）。

真实运行命令与结果：

```
cd units/ranges && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
EXIT=0
```

## 10. `units/alpha/` — `alpha.py`

改动文件：`units/alpha/alpha.py`（`add` 由 `a - b` 改为 `a + b`）

真实运行命令与结果：

```
cd units/alpha && python3 check.py
alpha ok
EXIT=0
```

## 11. `units/beta/` — `beta.py`

改动文件：`units/beta/beta.py`（`mul` 由 `a + b` 改为 `a * b`）

真实运行命令与结果：

```
cd units/beta && python3 check.py
beta ok
EXIT=0
```

## 12. `units/chain/` — `stage1`~`stage4`

改动文件：`units/chain/stage1.py`、`stage2.py`、`stage3.py`、`stage4.py`（tests/、data/ 未动）

语义边界：

- `stage1.parse_orders(path)`：`csv.DictReader` 读表头，每行所有字段 `int()`，返回 dict 列表。
- `stage2.filter_orders(rows)`：丢弃 `qty == 0` 的行。
- `stage3.total_by_price(rows)`：按 `price` 汇总 `qty`，返回 `{price: total}`。
- `stage4.report(totals)`：按 price 升序生成 `"price:total\n"` 拼接串。

端到端实测：

```
parse_orders('units/chain/data/orders.csv')
 = [{'id':1,'qty':2,'price':10},{'id':2,'qty':0,'price':5},
    {'id':3,'qty':3,'price':7},{'id':4,'qty':1,'price':7}]
report(total_by_price(filter_orders(rows))) == '7:4\n10:2\n'
```

真实运行命令与结果：

```
cd units/chain && python3 -m pytest -q tests/
....                                                                     [100%]
4 passed in 0.01s
EXIT=0
```

---

## 汇总复跑（leader 独立执行，非 worker 转述）

一条命令循环复跑全部 12 个单元的验收，结果：

| 单元 | 命令 | 结果 |
|------|------|------|
| csvfix | `python3 -m pytest -q` | 2 passed |
| rules | `python3 -m pytest -q` | 1 passed |
| ledger | `python3 -m pytest -q` | 2 passed |
| schedule | `python3 -m pytest -q` | 2 passed |
| intervals | `python3 -m pytest -q` | 7 passed |
| flags | `python3 -m pytest -q` | 8 passed |
| measure | `python3 -m pytest -q` | 1 passed |
| slugify | `python3 -m pytest -q` | 1 passed |
| ranges | `python3 -m pytest -q` | 1 passed |
| alpha | `python3 check.py` | `alpha ok` |
| beta | `python3 check.py` | `beta ok` |
| chain | `python3 -m pytest -q tests/` | 4 passed |

全部 `EXIT=0`。
