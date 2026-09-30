# 12 块独立交付物 —— 实施与验收报告

- 工作目录：`/home/rimuru/Projects/Code/for_fun/TeamAgents/review/eval/r2-p6/runs/2026-09-30-paratera-team/work/twelve-deliverables.D.1`
- 运行环境：Python 3.13.15，`python3 -m pytest`（pytest 可用）
- 组织方式：12 个互相独立的单元由 12 个 worker 各负责一块（每块一个独立验收），完成后由我统一独立复跑全部验收命令并汇总。
- **未修改任何测试/验收文件**：`units/*/test_*.py`、`units/chain/tests/test_stage*.py`、`units/alpha/check.py`、`units/beta/check.py`、`units/chain/data/orders.csv` 的文件修改时间仍为 2026-09-24/09-28（改动前）；只有实现文件变为 2026-09-30 21:34。

## 验收结果总览

| # | 单元 | 改动文件 | 验收命令 | 结果 |
|---|------|----------|----------|------|
| 1 | csvfix | `units/csvfix/impl.py` | `cd units/csvfix && python3 -m pytest -q` | 2 passed, exit 0 |
| 2 | rules | `units/rules/engine.py` | `cd units/rules && python3 -m pytest -q` | 1 passed, exit 0 |
| 3 | ledger | `units/ledger/ledger.py` | `cd units/ledger && python3 -m pytest -q` | 2 passed, exit 0 |
| 4 | schedule | `units/schedule/schedule.py` | `cd units/schedule && python3 -m pytest -q` | 2 passed, exit 0 |
| 5 | intervals | `units/intervals/intervals.py` | `cd units/intervals && python3 -m pytest -q` | 7 passed, exit 0 |
| 6 | flags | `units/flags/flags.py` | `cd units/flags && python3 -m pytest -q` | 8 passed, exit 0 |
| 7 | measure | `units/measure/measure.py` | `cd units/measure && python3 -m pytest -q` | 1 passed, exit 0 |
| 8 | slugify | `units/slugify/slugify.py` | `cd units/slugify && python3 -m pytest -q` | 1 passed, exit 0 |
| 9 | ranges | `units/ranges/ranges.py` | `cd units/ranges && python3 -m pytest -q` | 1 passed, exit 0 |
| 10 | alpha | `units/alpha/alpha.py` | `cd units/alpha && python3 check.py` | 输出 `alpha ok`, exit 0 |
| 11 | beta | `units/beta/beta.py` | `cd units/beta && python3 check.py` | 输出 `beta ok`, exit 0 |
| 12 | chain | `units/chain/stage1..4.py` | `cd units/chain && python3 -m pytest -q tests/` | 4 passed, exit 0 |

以上 12 条命令均由我在 worker 全部结算后，于工作目录逐条真实复跑得到。

## 逐块语义边界

### 1. `units/csvfix/impl.py`
- 实现：`parse(line)` 对每个字段 `strip()`；切分后列数不为 3 时返回 `None`；正好 3 列时返回去空白后的列表。
- 边界：整行由逗号切分；空行得到 `['']`（1 列）→ `None`；含引号的 CSV 不做引号解析（按测试语义，仅按逗号切分 + 裁剪）。
- 实现：`total(rows)` 累加第 **3** 列（索引 2）的整数值；第 3 列缺失或为空白（`strip()` 后为空）时跳过、按 0 计且不报错。
- 边界：第 3 列非整数（如 `"x"`）会抛 `ValueError`（测试未覆盖）。

### 2. `units/rules/engine.py`
- 实现：`evaluate(rules, facts)`；存在 `"all"` 键时返回 `all(bool(facts.get(k)) for k in ...)`；否则存在 `"any"` 键时用 `any(...)`。
- 边界：缺失的 fact 键视为 falsy；`facts` 中的值做真值判定；两者都缺时返回 `False`；同时含 `all` 与 `any` 时 `all` 优先（测试未覆盖）。

### 3. `units/ledger/ledger.py`
- 实现：`Ledger.apply(rows, op)` 先扫描全部行，任一金额 `< 0` 抛 `ValueError`；随后返回 `row[1] == op` 的行并保持原顺序。
- 边界：**负额校验作用于全部行**，即使该行不属于 `op` 账户也会抛错（与测试一致，测试未区分账户）。
- 实现：`balance(rows, account)` 对账户匹配行 `deposit` 累加、`withdraw` 累减，无匹配返回 0。
- 边界：未知 `kind` 被静默忽略（文档约定 kind 只有两种）。

### 4. `units/schedule/schedule.py`
- 实现：`slots(ranges, minutes)` 先按起点排序；当 `lo - cur_hi < minutes` 时合并（含区间重叠/相接导致的负间隔），否则另起一段；返回按起点升序的元组列表；空输入返回 `[]`。
- 边界：合并条件为严格 `< minutes`（空隙恰好等于 minutes 不合并）；`minutes` 为负值语义未定义/未测试。
- 实现：`overlaps(a, b)` 返回 `a[0] < b[1] and b[0] < a[1]`，端点恰好相接不算交叠。

### 5. `units/intervals/intervals.py`
- 实现 `merge(ranges, gap=0)`：对每段规范端序（`lo > hi` 时交换）、按 `(lo, hi)` 排序；当 `next.lo - prev.hi - 1 <= gap`（缺失整数个数 ≤ gap）时合并，否则保留；用 `hi` 最大值吸收嵌套段；返回升序元组列表，空输入 `[]`。
- 实现 `subtract(ranges, hole)`：先用 `merge(ranges)`（gap=0）归一/合并输入；与 hole 完全不相交的段原样保留；被完全覆盖的段消失；与 hole 相交的段按 `lo < hlo` 取左残段、`hi > hhi` 取右残段（`hlo-1` / `hhi+1` 为闭区间边界），返回升序结果。
- 实现 `total_length(ranges)`：对合并后的区间求 `sum(hi - lo + 1)`，重叠只计一次；空输入 0。
- 边界：端点为整数；减法使用归一化后的输入（测试 `test_subtract_normalizes_its_input_first` 覆盖）。

### 6. `units/flags/flags.py`
- 实现：顺序扫描 `argv`，返回 `{"values", "flags", "positional"}`。
  - `"--"` 置终止态，其后所有项进 `positional`（`"--"` 本身不出现）。
  - `"-"` 或非 `-` 开头 → `positional`。
  - `--key=value` 与 `--key value`（后一项不以 `--` 开头时才消费）→ `values[key].append(value)`。
  - `--key` 无可用值（行尾，或后一项以 `--` 开头）→ `flags[key] = True`。
  - `-abc` → 拆成 `a,b,c` 三个开关 True。
- 边界：重复 value 键按出现顺序累积成列表；重复开关仍为 `True`；`values` 可形如开关（`--k -5` → `"-5"`，`--k=--v` → `"--v"`）；未定义单横线长选项/`-k=v` 等形式的语义（`-key` 会被当作三连开关）。

### 7. `units/measure/measure.py`
- 实现：`Impl.to_cm(value, unit)` 用因子表 `{"m":100,"cm":1,"mm":0.1}`，返回 `value * factor`；未知单位抛 `ValueError`。
- 边界：`m` 返回整数倍（如 `1.5*100 == 150`）；`mm` 用浮点因子。

### 8. `units/slugify/slugify.py`
- 实现：`str(text).lower()`；`re.sub(r"[^a-zA-Z0-9]+", "-", ...)` 把连续非字母数字折成一个 `-`；`.strip("-")` 去首尾；结果为空返回 `"untitled"`。
- 边界：字母数字限定为 ASCII；下划线、Unicode 字母等均被视作分隔符（文档只要求“非字母数字”）。

### 9. `units/ranges/ranges.py`
- 实现：`Impl.merge(ranges)` 按起点排序后，当 `start <= last_end` 时合并（取较大 end），否则另起；返回按起点升序的 `(start, end)` 元组列表，空输入 `[]`。
- **语义边界（重要）**：合并条件是“有重叠（`start <= last_end`）”，而不是“整数相邻”。因此 `[1,4]` 与 `[5,7]` 保持分开——这正是测试 `merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]` 的要求。实现文件 docstring 中“adjacent”一词与实际/测试语义略有出入，以测试为准。

### 10. `units/alpha/alpha.py`
- 实现：修正 `add(a, b)`，由错误的 `a - b` 改为 `a + b`。`check.py` 未改动。

### 11. `units/beta/beta.py`
- 实现：修正 `mul(a, b)`，由错误的 `a + b` 改为 `a * b`。`check.py` 未改动。

### 12. `units/chain/stage1..4.py`
- `stage1.parse_orders(path)`：用 `csv.DictReader` 读取，跳过表头，每行转为 `{"id": int, "qty": int, "price": int}` 的列表。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行。
- `stage3.total_by_price(rows)`：按 `price` 聚合 `qty` 之和，返回 `dict`。
- `stage4.report(totals)`：按 price 升序输出 `"price:total\n"` 拼接字符串。
- 边界：CSV 数值列必须为可转 `int` 的整数；空输入聚合得 `{}`，报告得 `""`。

## 复现所有验收（一次运行）

```bash
cd /home/rimuru/Projects/Code/for_fun/TeamAgents/review/eval/r2-p6/runs/2026-09-30-paratera-team/work/twelve-deliverables.D.1
for u in csvfix rules ledger schedule intervals flags measure slugify ranges; do (cd units/$u && python3 -m pytest -q); done
(cd units/alpha && python3 check.py)
(cd units/beta && python3 check.py)
(cd units/chain && python3 -m pytest -q tests/)
```

## 未验证 / 需注意

- 12 个单元的全部验收标准均已真实复跑并全部通过；上述“边界”中标注为“测试未覆盖”的行为（如负额校验作用范围、`rules` 同时含 `all`/`any`、`slots` 的负 minutes、非整数 CSV 字段等）没有对应测试，属于依文档实现但未由验收断言确认的语义。
- 测试通过由 `pytest -q` 的退出码 0 与计数确认；未运行除上述命令之外的其他测试或静态检查。
