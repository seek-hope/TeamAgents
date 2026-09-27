# REPORT — 12 个独立交付物

日期：2026-09-24　工作目录：`work/twelve-deliverables.B.2`

本文件逐块说明实现的语义边界，以及**真实运行过**的命令与结果。
所有测试/验收文件（`test_*.py`、`check.py`）均未修改；只改动了 12 个实现文件。

## 汇总结果

在根目录下逐单元运行（每次 `cd` 到对应单元），全部通过：

| # | 单元 | 命令 | 结果 |
|---|------|------|------|
| 1 | `units/csvfix` | `cd units/csvfix && python3 -m pytest -q` | `2 passed` |
| 2 | `units/rules` | `cd units/rules && python3 -m pytest -q` | `1 passed` |
| 3 | `units/ledger` | `cd units/ledger && python3 -m pytest -q` | `2 passed` |
| 4 | `units/schedule` | `cd units/schedule && python3 -m pytest -q` | `2 passed` |
| 5 | `units/intervals` | `cd units/intervals && python3 -m pytest -q` | `7 passed` |
| 6 | `units/flags` | `cd units/flags && python3 -m pytest -q` | `8 passed` |
| 7 | `units/measure` | `cd units/measure && python3 -m pytest -q` | `1 passed` |
| 8 | `units/slugify` | `cd units/slugify && python3 -m pytest -q` | `1 passed` |
| 9 | `units/ranges` | `cd units/ranges && python3 -m pytest -q` | `1 passed` |
| 10 | `units/alpha` | `cd units/alpha && python3 check.py` | `alpha ok` |
| 11 | `units/beta` | `cd units/beta && python3 check.py` | `beta ok` |
| 12 | `units/chain` | `cd units/chain && python3 -m pytest -q tests/` | `4 passed` |

最后一轮 12 条命令的汇总输出：`ALL_OK=yes`（12/12 退出码为 0）。

## 逐块语义边界

### 1. `units/csvfix/impl.py`（改：`impl.py`）
- `parse(line)`：用 `csv.reader([line], skipinitialspace=True)` 解析单行；逐字段 `strip()`；
  双引号包裹字段（含字段内逗号）被正确识别，即使引号前有空白；**字段数不是 3 时返回 `None`**。
  例：`' a , 2 , x ' -> ['a','2','x']`；`' "a,b" , 2 , x ' -> ['a,b','2','x']`；`'a,2' -> None`。
- `total(rows)`：把**每行第三列**当金额求和；跳过列数不足的行和空字段（不视为 0，也不报错）。
  例：`[["a","1","2"],["b","2",""]] -> 2`。非数字非空字段会由 `int()` 抛 `ValueError`。

### 2. `units/rules/engine.py`（改：`engine.py`）
- `evaluate(rules, facts)`：支持 `{"all":[...]}`（全部为真才 True）和 `{"any":[...]}`（任一为真即 True）；
  两者同时存在时取逻辑与；缺失的键按假处理；返回真正的 `bool`。
- 空规则 `{}` 返回 `True`（没有约束）。

### 3. `units/ledger/ledger.py`（改：`ledger.py`）
- `Ledger.apply(rows, op)`：遍历全部行做校验，`kind` 不属于 `deposit`/`withdraw` 或金额为负
  立即抛 `ValueError`（先校验后过滤）；返回 `account == op` 的行，保持原顺序。
- `balance(rows, account)`：`deposit` 加、`withdraw` 减；无该账户的行返回 `0`；未知 `kind` 抛 `ValueError`。

### 4. `units/schedule/schedule.py`（改：`schedule.py`）
- `slots(ranges, minutes)`：按 lo 排序后合并，**当 `next_lo - prev_hi < minutes` 时才合并**
  （等于 `minutes` 不合并）；返回按起点升序的元组列表，空输入 `[]`。
  例：`slots([(0,60),(30,90),(120,150)], 30) == [(0,90),(120,150)]`（90→120 空隙正好 30，不合并）。
- `overlaps(a, b)`：`max(lo) < min(hi)`；端点相接不算交叠，如 `(0,10)` 与 `(10,20)` 为 `False`。

### 5. `units/intervals/intervals.py`（改：`intervals.py`）
- `merge(ranges, gap=0)`：按 lo 排序；当相邻两段缺失整数个数 `next.lo - prev.hi - 1 <= gap` 时合并，
  合并取较大的 hi（嵌套区间被吸收）；返回升序元组列表。
  例：`merge([[5,7],[1,3],[-1,0]]) == [(-1,3),(5,7)]`；`merge([[1,2],[4,5]],1)==[(1,5)]`。
- `subtract(ranges, hole)`：先对输入做一次 `merge(ranges)` 归一化，再逐段挖洞：
  不相交的段原样保留、被完全覆盖的段消失、横跨的段按 `lo..hole.lo-1` 与 `hole.hi+1..hi` 分裂（空侧不产出）。
  例：`subtract([[0,10]],(4,6)) == [(0,3),(7,10)]`；`subtract([[5,7],[1,3]],(2,6)) == [(1,1),(7,7)]`。
- `total_length(ranges)`：先 merge 去重，再对每段累加 `hi - lo + 1`（闭区间含端点，重叠只算一次）。

### 6. `units/flags/flags.py`（改：`flags.py`）
- `parse(argv)` 返回 `{"values": {...}, "flags": {...}, "positional": [...]}`。
- 不以 `-` 开头、或正好是 `-` 的项按顺序进 `positional`。
- 遇到 `--` 时其后的项全部进 `positional`，`--` 自身不出现。
- `--key=value` 与 `--key value` 都记入 `values[key].append(value)`。
- `--key` 后无可取值（到结尾，或下一项以 `--` 开头）时记 `flags[key]=True`。
- 单短横线 `-abc` 展开为开关 `a`、`b`、`c`。
- `values` 的值恒为列表、重复键按出现顺序累积；开关重复仍为 `True`；
  同一个键先前作值、后来作开关时，`values` 与 `flags` 中会同时存在（与测试一致）。

### 7. `units/measure/measure.py`（改：`measure.py`）
- `Impl.to_cm(value, unit)`：`m -> *100`、`cm -> *1`、`mm -> *0.1`；返回浮点数（`150.0 == 150`）。
- 其它单位抛 `ValueError`。例：`to_cm(1.5,"m")==150`，`to_cm(20,"mm")==2`，`to_cm(1,"ft")` 抛错。

### 8. `units/slugify/slugify.py`（改：`slugify.py`）
- `Impl.slugify(text)`：`str(text).lower()`；`re.sub(r"[^a-z0-9]+","-",...)` 折叠非 ASCII 字母数字；
  `.strip("-")` 去首尾连字符；空结果返回 `"untitled"`。
  例：`"Hello, World!" -> "hello-world"`，`"  --A@@B-- " -> "a-b"`，`"***" -> "untitled"`。

### 9. `units/ranges/ranges.py`（改：`ranges.py`）
- `Impl.merge(ranges)`：排序后仅当真正重叠或端点接触（`next.lo <= cur.hi`）时合并；
  相邻但不相交（如 `(1,4)` 与 `(5,7)`，缺失整数 0 个但不相交）保持分离。返回升序元组列表。
  例：`merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]`。

### 10. `units/alpha/alpha.py`（改：`alpha.py`）
- `add(a, b)` 修正为 `a + b`（原为 `a - b`）。

### 11. `units/beta/beta.py`（改：`beta.py`）
- `mul(a, b)` 修正为 `a * b`（原为 `a + b`）。

### 12. `units/chain/`（改：`stage1.py`、`stage2.py`、`stage3.py`、`stage4.py`）
- `stage1.parse_orders(path)`：用 `csv.DictReader` 读 CSV，把 `id/qty/price` 转成 `int`，
  返回 `[{"id":...,"qty":...,"price":...}, ...]`。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行。
- `stage3.total_by_price(rows)`：按 `price` 分组合计 `qty`，返回 `{price: 总数量}`（插入序为首次出现序）。
- `stage4.report(totals)`：按 `price` 升序渲染 `"price:qty\n"` 文本，如 `{10:2,7:4} -> "7:4\n10:2\n"`。

## 验证方式与未验证声明

- 已运行：上表 12 条命令（逐单元 `cd` 后执行），退出码全为 0；另附边界探针（quoted CSV、
  缺失整数合并、`--k -5`、`--` 终止符、`subtract` 分裂、非法单位抛错等）均符合预期。
- 未改动任何 `test_*.py` / `check.py`；`ls` 时间戳显示这些文件早于本次实现文件。
- 未验证/未覆盖：测试用例之外的输入（如负数区间端点、非整数金额、Unicode slug、短选项带 `=` 等）
  未做正式断言，只按文档字符串的字面语义实现。
