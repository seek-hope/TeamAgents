# 12 个独立交付物 — 集成报告

本报告逐块说明实现的语义边界，以及**真实运行过**的命令与结果。
所有 12 块互相独立；每块由一名独立 worker 完成，随后由集成方（本实例）亲自重跑全部验收命令。

测试/验收文件（`test_*.py`、`check.py`）一律未改动。

---

## 总览：验收命令与结果（集成方亲自重跑）

| # | 单元 | 命令 | 结果 | exit |
|---|------|------|------|------|
| 1 | csvfix | `cd units/csvfix && python3 -m pytest -q` | `2 passed in 0.03s` | 0 |
| 2 | rules | `cd units/rules && python3 -m pytest -q` | `1 passed in 0.01s` | 0 |
| 3 | ledger | `cd units/ledger && python3 -m pytest -q` | `2 passed in 0.01s` | 0 |
| 4 | schedule | `cd units/schedule && python3 -m pytest -q` | `2 passed in 0.01s` | 0 |
| 5 | intervals | `cd units/intervals && python3 -m pytest -q` | `7 passed in 0.01s` | 0 |
| 6 | flags | `cd units/flags && python3 -m pytest -q` | `8 passed in 0.03s` | 0 |
| 7 | measure | `cd units/measure && python3 -m pytest -q` | `1 passed in 0.01s` | 0 |
| 8 | slugify | `cd units/slugify && python3 -m pytest -q` | `1 passed in 0.01s` | 0 |
| 9 | ranges | `cd units/ranges && python3 -m pytest -q` | `1 passed in 0.01s` | 0 |
| 10 | alpha | `cd units/alpha && python3 check.py` | `alpha ok` | 0 |
| 11 | beta | `cd units/beta && python3 check.py` | `beta ok` | 0 |
| 12 | chain | `cd units/chain && python3 -m pytest -q tests/` | `4 passed in 0.04s` | 0 |

---

## 1. `units/csvfix/impl.py` — parse / total

**parse(line)**
- 以 `,` 切分；**字段数不等于 3 时返回 `None`**（过少或过多都拒绝）。
- 恰好 3 列时，对每个字段做 `strip()` 后返回三元列表。
- 边界：不做引号感知切分（`"a,b",c,d` 不按 CSV 引号规则处理），也没有类型转换，结果始终是字符串。

**total(rows)**
- 按**第 3 列（索引 2）**求和，`int` 转换。
- 金额为空（`""`/纯空白/`None`）或整行不足 3 列时跳过该行，不计入也不报错。
- 非数字金额会自然抛 `ValueError`（未捕获）。

> **语义冲突说明**：委派描述一度写成“求第 2 列”，但验收测试
> `test_total_skips_blank_and_reports_bad` 给的是 `[["a","1","2"],["b","2",""]]` 且要求 `== 2`；
> 求第 2 列应为 `1+2=3`，与测试矛盾。故以测试为准：**求第 3 列并跳过空金额**。
> 该实现下的真实命令与结果：`2 passed`。

## 2. `units/rules/engine.py` — evaluate(rules, facts)

- `{"all": [k...]}`：键为真值才为真；**空列表为真（vacuous True）**。
- `{"any": [k...]}`：任一键为真即为真；**空列表为假**。
- 缺失键视为假；返回 `bool`。
- 边界：不支持 `all`/`any` 同时出现时的组合语义（优先 `all`）；既无 `all` 也无 `any` 的规则抛 `ValueError`。

## 3. `units/ledger/ledger.py` — Ledger.apply / balance

- `apply(rows, op)`：先**整批校验**，任意行的 `cents < 0` 就抛 `ValueError`（整次调用拒绝，不返回部分结果）；
  校验通过后返回 `account == op` 的行，保持原始顺序，元素仍是三元组。
- `balance(rows, account)`：对该账户 `deposit` 累加、`withdraw` 累减；无匹配行返回 `0`。
- 边界：对未知 `kind` 静默忽略（不参与余额）；重复调用 `apply` 无状态（类本身不保存状态）。

## 4. `units/schedule/schedule.py` — slots / overlaps

- `slots(ranges, minutes)`：按 `lo` 升序排序后线性合并；当 **`next_lo - prev_hi < minutes`** 时并入同一段（取 `hi` 的较大者）。返回按起点升序的元组列表；空输入返回 `[]`。
- 注意这里判据是“间隙严格小于”，而 `units/intervals` 用的是“缺失整数个数 <= gap”，两者语义不同、各自独立。
- `overlaps(a, b)`：闭区间正重叠才为真；**端点相接不算重叠**（`(0,10)` 与 `(10,20)` 为 `False`）。
- 边界：`slots` 假定输入已是 `lo <= hi`，不做反向或嵌套归一化之外的额外处理（嵌套因取 `max hi` 自然收敛）。

## 5. `units/intervals/intervals.py` — Impl.merge / subtract / total_length

- 统一入口 `_normalize`：把每段转成 `int` 元组并按 `lo` 升序排序。
- `merge(ranges, gap=0)`：相邻两段之间**缺失整数个数** `next.lo - prev.hi - 1 <= gap` 时合并。默认 `gap=0`，因此相邻（缺 0 个）与重叠/嵌套都合并，中间缺 1 个整数则不合并。
- `subtract(ranges, hole)`：先对 `ranges` 做 `merge` 归一化；与 `hole` 不相交的段原样保留；被完全覆盖的段消失；跨越 `hole` 的段按 `(lo, hlo-1)` / `(hhi+1, hi)` 分裂，端点相接不会产生空段。
- `total_length(ranges)`：`sum(hi - lo + 1)`，基于合并后的区间，**重叠只算一次**。
- 边界：要求整数端点；`subtract` 的 `hole` 假定 `lo <= hi`。

## 6. `units/flags/flags.py` — Impl.parse(argv)

返回 `{"values": {...}, "flags": {...}, "positional": [...]}`。
- `--`：终止解析，其后所有项按原顺序进 `positional`，`--` 本身不出现。
- `--key=value`：以第一个 `=` 切分，`value` 原样保留（可形如 `--v`）。`--key value`：仅当下一个项**不以 `--` 开头**时作为值被消费（故 `-5` 可作值，而 `--other` 不行）。否则该键成为开关 `flags[key]=True`。
- `-abc`：拆成三个短开关 `a`、`b`、`c`。单个 `-` 与任何非 `-` 开头项进 `positional`。
- `values` 的值恒为**列表**，重复键按遇到顺序追加；重复开关保持 `True`。
- 边界：仅识别 `--` 前缀（不处理 Windows 风格 `/x`）；`-abc` 不区分“短开关带值”；未知的形如 `-x` 的长选项按单字符短开关逐字拆。

## 7. `units/measure/measure.py` — Impl.to_cm(value, unit)

- 系数表 `m=100`、`cm=1`、`mm=0.1`；返回 `value * factor`。
- 非法单位抛 `ValueError`（`KeyError` 被转成 `ValueError`）。
- 边界：返回类型随输入而定；`mm` 因乘 `0.1` 会返回浮点（如 `to_cm(20,"mm") == 2.0`，与 `2` 数值相等，测试 `== 2` 通过）。不做单位大小写归一化。

## 8. `units/slugify/slugify.py` — Impl.slugify(text)

- `str(text).lower()` 后，用 `[^a-z0-9]+` 折叠成单个 `-`，再 `strip("-")`；结果为空则返回 `"untitled"`。
- 边界：非 ASCII 字母（如中文、重音字母）会被视为分隔符；连续分隔符折叠为一个 `-`；数字保留。

## 9. `units/ranges/ranges.py` — Impl.merge(ranges)

- 先把每段规范为 `(start, end)`（`start > end` 时交换），按起点排序，再合并 `start <= 当前 end`（重叠或**相接**，如 `[1,3]` 与 `[3,4]`）的区间，取较大 `end`。
- 返回按起点升序的**元组列表**；空输入 `[]`。
- 边界：与 `units/intervals` 的 `gap` 语义不同，这里没有 gap 参数，相接即合并（等价 `gap=0` 时的“缺 0 个整数合并”，但 `[1,2]` 与 `[3,4]` 因缺 0 个整数也会合并；而 `ranges` 测试只覆盖重叠/相接）。反向区间会被交换而非报错。

## 10. `units/alpha/alpha.py`

- `add` 原为 `a - b`（错误），改为 `a + b`。`check.py` 未改。

## 11. `units/beta/beta.py`

- `mul` 原为 `a + b`（错误），改为 `a * b`。`check.py` 未改。

## 12. `units/chain/` — stage1..stage4

四段流水线，各自独立可测：
- `stage1.parse_orders(path)`：用 `csv.DictReader` 读带表头 `id,qty,price` 的 CSV，逐行转 `int`，返回 `[{"id","qty","price"}, ...]`。边界：表头名必须精确匹配；非整数会抛 `ValueError`；不做缺列容错。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行（`qty == 0` 被滤掉）。
- `stage3.total_by_price(rows)`：按 `price` 分组累加 `qty`，返回 `{price: total}`。
- `stage4.report(totals)`：按 `price` 升序输出 `"price:qty\n"` 拼接的字符串（升序来自 `sorted(totals)`）。
- 边界：`report` 不排序等于稳定排序的输入也无妨；四段互不耦合，仅靠 dict 结构衔接。

---

## 复现方式（集成方实际执行）

```bash
cd units/csvfix    && python3 -m pytest -q          # 2 passed
cd units/rules     && python3 -m pytest -q          # 1 passed
cd units/ledger    && python3 -m pytest -q          # 2 passed
cd units/schedule  && python3 -m pytest -q          # 2 passed
cd units/intervals && python3 -m pytest -q          # 7 passed
cd units/flags     && python3 -m pytest -q          # 8 passed
cd units/measure   && python3 -m pytest -q          # 1 passed
cd units/slugify   && python3 -m pytest -q          # 1 passed
cd units/ranges    && python3 -m pytest -q          # 1 passed
cd units/alpha     && python3 check.py              # alpha ok
cd units/beta      && python3 check.py              # beta ok
cd units/chain     && python3 -m pytest -q tests/   # 4 passed
```

## 改动文件清单

- `units/csvfix/impl.py`、`units/rules/engine.py`、`units/ledger/ledger.py`、`units/schedule/schedule.py`、
  `units/intervals/intervals.py`、`units/flags/flags.py`、`units/measure/measure.py`、`units/slugify/slugify.py`、
  `units/ranges/ranges.py`、`units/alpha/alpha.py`、`units/beta/beta.py`、
  `units/chain/stage1.py`、`units/chain/stage2.py`、`units/chain/stage3.py`、`units/chain/stage4.py`
- 新增：`REPORT.md`（本文件）
- **未改动**任何 `test_*.py` / `check.py`。

## 未验证 / 存疑项

- `measure.to_cm(20, "mm")` 返回 `2.0`（浮点）而非整数 `2`；测试用 `==` 判定通过，若下游要求严格 `int` 类型需另行归一。
- `rules.evaluate` 对同时含 `all` 与 `any` 的规则只取 `all`，该组合未被测试覆盖。
- `intervals.merge` 与 `ranges.merge` 的“相接”语义各自独立，未被要求相互一致。
