# 12 个独立交付物 —— 实现报告

本报告逐块说明实现的语义与边界，并列出**真实运行过**的命令与结果。
所有验收/测试文件均未修改（文末附前后一致的 sha256）。

## 汇总：验收命令与结果

| # | 单元 | 命令（在对应目录下执行） | 结果 |
|---|------|--------------------------|------|
| 1 | csvfix | `python3 -m pytest -q` | `2 passed`，EXIT=0 |
| 2 | rules | `python3 -m pytest -q` | `1 passed`，EXIT=0 |
| 3 | ledger | `python3 -m pytest -q` | `2 passed`，EXIT=0 |
| 4 | schedule | `python3 -m pytest -q` | `2 passed`，EXIT=0 |
| 5 | intervals | `python3 -m pytest -q` | `7 passed`，EXIT=0 |
| 6 | flags | `python3 -m pytest -q` | `8 passed`，EXIT=0 |
| 7 | measure | `python3 -m pytest -q` | `1 passed`，EXIT=0 |
| 8 | slugify | `python3 -m pytest -q` | `1 passed`，EXIT=0 |
| 9 | ranges | `python3 -m pytest -q` | `1 passed`，EXIT=0 |
| 10 | alpha | `python3 check.py` | `alpha ok`，EXIT=0 |
| 11 | beta | `python3 check.py` | `beta ok`，EXIT=0 |
| 12 | chain | `python3 -m pytest -q tests/` | `4 passed`，EXIT=0 |

合计 29 个 pytest 用例全部通过，alpha/beta 两个 `check.py` 均打印 ok。原始输出保存在
`acceptance.log`（由本文末尾给出的批量脚本真实生成）。

批量执行脚本（工作区根目录下运行）：

```bash
ROOT=.
for d in csvfix rules ledger schedule intervals flags measure slugify ranges; do
  echo "===== units/$d : python3 -m pytest -q ====="
  (cd "$ROOT/units/$d" && python3 -m pytest -q); echo "EXIT=$?"
done
(cd units/alpha && python3 check.py); echo "EXIT=$?"
(cd units/beta  && python3 check.py); echo "EXIT=$?"
(cd units/chain && python3 -m pytest -q tests/); echo "EXIT=$?"
```

---

## 1. `units/csvfix/impl.py`（`parse` / `total`）

改动文件：`units/csvfix/impl.py`。

**`parse(line)` 语义**
- 用标准库 `csv.reader` 解析**单条**记录：双引号包裹的字段可含逗号，`""` 表示字面双引号。
- 每个字段两侧空白被裁剪（`.strip()`）。
- 列数必须**恰好 3**，否则返回 `None`（不抛异常）。
- 空串 / `None` 输入返回 `None`。

边界示例（真实运行验证）：`parse(' a , 2 , x ')` → `['a','2','x']`；`parse('a,2')` → `None`；
`parse('')` → `None`；`parse('"a,b", 2 ,x')` → `['a,b','2','x']`。

**`total(rows)` 语义**
- 累加每行**第三列**（金额列）的整数值。
- 金额为空串或 `None` 的行跳过（不当作 0，也不报错）。
- 金额非空但不是合法整数时 `int()` 自然抛出 `ValueError`。
- 列数少于 3 的行跳过。

边界示例（真实运行验证）：`total([["a","1","2"],["b","2",""]])` → `2`；
`total([["a","1","x"]])` → 抛 `ValueError`。

> 语义歧义说明：该文件 docstring 把“把第三列当成金额”写成 BUG，但冻结测试
> `test_total_skips_blank_and_reports_bad` 的期望值 `2` 只能由“累加第三列、空值跳过”得到
> （行 `["a","1","2"]` 贡献 2，行 `["b","2",""]` 因第三列为空被跳过），因此本实现以冻结测试为准。

## 2. `units/rules/engine.py`（`evaluate`）

改动文件：`units/rules/engine.py`。
- `{"all": [...]}`：所有键对应 fact 为真才返回 `True`。
- `{"any": [...]}`：任一键为真即返回 `True`。
- 缺失的键视为 `False`（用 `facts.get(key, False)`）。
- 返回的是真正的 `bool`（满足 `is True` / `is False`）。
- 两种形状都不匹配时返回 `False`；若同时出现 `"all"` 与 `"any"`，`"all"` 优先。

边界示例（真实运行验证）：`evaluate({"all":["a","zz"]}, {"a":True})` → `False`。

## 3. `units/ledger/ledger.py`（`Ledger.apply` / `balance`）

改动文件：`units/ledger/ledger.py`。
- `apply(rows, op)`：按原顺序返回 `kind == deposit/withdraw` 且 `account == op` 的行。
- 对**输入的所有行**做校验：金额为负 → `ValueError`；`kind` 不属于 deposit/withdraw → `ValueError`
  （即对非目标账户的非法行也会报错，这是刻意的严格行为）。
- `balance(rows, account)`：该账户 `deposit` 求和、`withdraw` 相减；无该账户行时返回 `0`。

边界示例（真实运行验证）：`balance([("deposit","a",100),("withdraw","a",30)], "a")` → `70`。

## 4. `units/schedule/schedule.py`（`slots` / `overlaps`）

改动文件：`units/schedule/schedule.py`。
- `slots(ranges, minutes)`：按起点排序后合并；相邻两段满足 `overlap`（`lo <= prev_hi`）或
  空隙 `next_lo - prev_hi < minutes` 时合并为同一段（取较大的 hi）。空输入返回 `[]`；
  返回按起点升序的 `(lo, hi)` 元组列表。
- `overlaps(a, b)`：闭区间是否有交叠，判据 `a0 < b1 and b0 < a1`；端点正好相接不算交叠。

边界示例（真实运行验证）：`slots([(0,60),(30,90),(120,150)], 30)` → `[(0,90),(120,150)]`
（120-90=30 不 `< 30` 故不合并）；`slots([(0,10),(40,50)], 30)` → `[(0,10),(40,50)]`；
`overlaps((0,10),(10,20))` → `False`。

## 5. `units/intervals/intervals.py`（`Impl`）

改动文件：`units/intervals/intervals.py`。
- `merge(ranges, gap=0)`：先把每段端点规范化为 `(min,max)`，按 lo 排序；相邻两段
  **缺失整数个数** `next.lo - prev.hi - 1 <= gap` 时合并（同时覆盖嵌套情形）。空输入返回 `[]`；
  返回按 lo 升序的元组列表。
- `subtract(ranges, hole)`：先对 `ranges` 做 `merge` 归一化；与 hole 不相交的段原样保留，
  被完全覆盖的段消失，横跨 hole 的段分裂为 `(lo, hole_lo-1)` 与 `(hole_hi+1, hi)`（端点包含）。
- `total_length(ranges)`：先 `merge` 去重，再对每段累加 `hi - lo + 1`（闭区间整数个数）。

边界示例（真实运行验证）：`merge([[1,2],[4,5]], 1)` → `[(1,5)]`；
`subtract([[5,7],[1,3]], (2,6))` → `[(1,1),(7,7)]`；`total_length([[0,2],[2,4]])` → `5`。
注意：`subtract` 的“归一化”会把重叠/相邻（gap=0）的输入段先合并，因此对这类输入不会“原样”保留。

## 6. `units/flags/flags.py`（`Impl.parse`）

改动文件：`units/flags/flags.py`。返回 `{"values": {...}, "flags": {...}, "positional": [...]}`。
- 不以 `-` 开头、或正好等于 `-` 的项按顺序进 `positional`。
- 恰好 `--`：其后所有项都进 `positional`，`--` 本身不出现在结果中。
- `--key=value`：`values[key]` 追加 `value`（`value` 可为任意字符串，含前导 `--`）。
- `--key`（无 `=`）：若存在下一项且下一项**不以 `--` 开头**，则把它当作值消费（因此 `-5`
  这样单破折号开头项也是值）；否则该键是开关，记入 `flags[key] = True`。
- `-abc`：拆成开关 `a`、`b`、`c`。
- `values` 的每个值是列表，重复键按出现顺序累积；重复开关保持 `True`。

边界示例（真实运行验证）：`parse(["--k","-5"])` → `values {"k":["-5"]}`；
`parse(["--k=--v"])` → `values {"k":["--v"]}`；
`parse(["--mode","--other"])` → `flags {"mode":True,"other":True}`；
`parse(["--","--x","y"])` → `positional ["--x","y"]`，`flags {}`。
未实现的语义：短选项不带值（如 `-o file` 只把 `-o` 记为开关，不会把 `file` 当值）。

## 7. `units/measure/measure.py`（`Impl.to_cm`）

改动文件：`units/measure/measure.py`。
- `to_cm(value, unit)`：`m` → `value*100`，`cm` → `value*1`，`mm` → `value*0.1`。
- 单位不在 `{m, cm, mm}` 中时抛 `ValueError`。

边界示例（真实运行验证）：`to_cm(20,"mm")` → `2.0`（`== 2`）；`to_cm(1,"ft")` → `ValueError`。

## 8. `units/slugify/slugify.py`（`Impl.slugify`）

改动文件：`units/slugify/slugify.py`。
- 先 `lower()`；把每个非 `[a-z0-9]` 连续片段折叠成一个 `-`；去首尾 `-`；结果为空则返回 `untitled`。
- 只把 ASCII 字母数字当“字母数字”，下划线、非 ASCII 字母都视作分隔符。

边界示例：`slugify("Hello, World!")` → `hello-world`；`slugify("  --A@@B-- ")` → `a-b`；
`slugify("***")` → `untitled`。

## 9. `units/ranges/ranges.py`（`Impl.merge`）

改动文件：`units/ranges/ranges.py`。
- 对闭区间排序后，仅在两段**重叠或共享端点**（`next.lo <= prev.hi`）时合并；返回按起点升序的
  元组列表；空输入返回 `[]`。
- 只差 1 的相邻但不重叠区间（如 `[1,1]` 与 `[2,2]`）**不**合并（真实运行验证返回两段）。

边界示例：`merge([[5,7],[1,3],[2,4]])` → `[(1,4),(5,7)]`；`merge([])` → `[]`；
`merge([[1,1]])` → `[(1,1)]`。

## 10. `units/alpha/alpha.py`

改动文件：`units/alpha/alpha.py`。把错误的减法改成加法：`add(a, b) = a + b`。
`python3 check.py` 输出 `alpha ok`。

## 11. `units/beta/beta.py`

改动文件：`units/beta/beta.py`。把错误的加法改成乘法：`mul(a, b) = a * b`。
`python3 check.py` 输出 `beta ok`。

## 12. `units/chain/`（`stage1` – `stage4`）

改动文件：`units/chain/stage1.py`、`stage2.py`、`stage3.py`、`stage4.py`。
- `stage1.parse_orders(path)`：用 `csv.DictReader` 读 `id,qty,price` 三列并转成 `int`，
  返回 `[{"id":..,"qty":..,"price":..}, ...]`。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行，顺序不变。
- `stage3.total_by_price(rows)`：按 `price` 归组累加 `qty`，返回 `{price: total_qty}`。
- `stage4.report(totals)`：按 price 升序渲染 `"{price}:{qty}\n"` 拼接。

端到端验证（真实运行）：`parse_orders("units/chain/data/orders.csv")` →
`[{1,2,10},{2,0,5},{3,3,7},{4,1,7}]`；整链 `report(total_by_price(filter_orders(...)))` →
`'7:4\n10:2\n'`，与各阶段测试一致。

---

## 未修改验收文件的证据

以下 sha256 在改动前后一致（csvfix 的哈希与仓库中另一运行记录的冻结哈希
`588db2b7...` 相同）：

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

## 额外边界探测（真实运行）

除验收套件外，还运行了一段独立脚本，逐项验证“报告里的语义边界”，观察到的输出包括：
`csvfix.parse('"a,b", 2 ,x') == ['a,b','2','x']`、`csvfix.total([["a","1","x"]])` 抛
`ValueError`、`schedule.slots([(0,10),(40,50)],30) == [(0,10),(40,50)]`、
`intervals.merge([[1,2],[4,5]],1) == [(1,5)]`、`ranges.merge([[1,1],[2,2]]) == [(1,1),(2,2)]`
（相邻不合并），以及 chain 端到端 `'7:4\n10:2\n'`。这些均为实际输出，不是推测。
