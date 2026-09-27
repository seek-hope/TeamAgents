# REPORT — 六个独立交付物

实现日期：2026-09-24。环境：Python 3.13.15，pytest 9.0.3。
测试文件一律未修改。六个单元各自在自身目录下运行（测试用裸 `import impl/engine/...`）。

## 汇总结果

| 单元 | 修改文件 | 命令 | 结果 |
| --- | --- | --- | --- |
| csvfix | `units/csvfix/impl.py` | `cd units/csvfix && python3 -m pytest -q` | 2 passed |
| rules | `units/rules/engine.py` | `cd units/rules && python3 -m pytest -q` | 1 passed |
| ledger | `units/ledger/ledger.py` | `cd units/ledger && python3 -m pytest -q` | 2 passed |
| schedule | `units/schedule/schedule.py` | `cd units/schedule && python3 -m pytest -q` | 2 passed |
| intervals | `units/intervals/intervals.py` | `cd units/intervals && python3 -m pytest -q` | 7 passed |
| flags | `units/flags/flags.py` | `cd units/flags && python3 -m pytest -q` | 8 passed |

一次性运行的等价命令：

```
for d in csvfix rules ledger schedule intervals flags; do
  (cd "units/$d" && python3 -m pytest -q)
done
```

输出（节选）：

```
===== csvfix =====    2 passed in 0.02s
===== rules =====     1 passed in 0.02s
===== ledger =====    2 passed in 0.01s
===== schedule =====  2 passed in 0.02s
===== intervals ===== 7 passed in 0.06s
===== flags =====     8 passed in 0.08s
```

## 1. csvfix (`impl.py`)

语义：
- `parse(line)`：按 RFC4180 风格切分——双引号包裹的字段可含逗号，`""` 表示字面双引号；
  未加引号的字段裁剪两侧空白，引号字段保留内部原样。列数必须恰好为 3，否则返回 `None`（不抛异常）。
- `total(rows)`：累加每行**第三列**（金额列）。金额为空字符串（或 `None`）的行跳过；
  金额存在但不是合法整数时抛 `ValueError`；行长度不足 3 列跳过。

边界：
- `' a , 2 , x '` → `["a","2","x"]`；`'a,2'` / `''` → `None`。
- `'"a,b",2,x'` → `["a,b","2","x"]`（引号处理的额外保证，测试未覆盖）。
- 空金额不计入且不报错；非法金额报错（`bad amount: 'x'`）。

说明：原 docstring 称“把第三列当成金额”是 bug，但冻结测试要求
`total([["a","1","2"],["b","2",""]]) == 2`，只有以第三列为金额并跳过空值才成立
（第二列会得到 3）。因此以测试为准，金额列取第三列。

## 2. rules (`engine.py`)

语义 `evaluate(rules, facts)`：
- `{"all": [...]}`：列表中所有 key 在 `facts` 中为真 → `True`；空列表 → `True`。
- `{"any": [...]}`：至少一个为真 → `True`；空列表 → `False`。
- 同时给出 all 与 any：两者都需满足（逻辑与）。
- 缺失的 key 视为假；返回值是真正的 `bool`（测试用 `is True/is False`）。
- 既无 all 也无 any → `False`。

## 3. ledger (`ledger.py`)

- `Ledger.apply(rows, op)`：`op` 是被筛选的账户名。返回 `row[1] == op` 的行，保持原顺序。
  校验**所有行**：金额为负抛 `ValueError`；kind 不在 `{"deposit","withdraw"}` 也抛 `ValueError`。
- `balance(rows, account)`：只统计该账户的行，`deposit` 相加、`withdraw` 相减；
  无该账户行返回 0；匹配到的行 kind 非法时抛 `ValueError`。

边界：筛选出的账户不存在返回 `[]`；`balance` 对不存在的账户返回 `0`；
非目标账户的负金额同样会让 `apply` 抛错（校验在筛选前进行）。

## 4. schedule (`schedule.py`)

- `slots(ranges, minutes)`：闭区间合并。按起点升序；相邻两段的空隙 `next_lo - prev_hi`
  严格小于 `minutes` 时合并（重叠/相接空隙 <= 0 恒合并），合并时取较大的 `hi`（含嵌套区间）。
  返回升序元组列表；空输入返回 `[]`。
- `overlaps(a, b)`：闭区间交叠判定为 `a.lo < b.hi and b.lo < a.hi`，
  因此端点相接 `(0,10)` 与 `(10,20)` 不算交叠；返回 `bool`。

## 5. intervals (`intervals.py`)

- `merge(ranges, gap=0)`：按 `lo` 升序；相邻缺失整数个数 `next.lo - prev.hi - 1 <= gap`
  时合并，取较大 `hi`。空输入返回 `[]`。
- `subtract(ranges, hole)`：先 `merge` 归一化输入，再逐段挖洞。
  与 hole 不相交（含仅端点相接）的段原样保留；覆盖左端的留下右段，覆盖右端的留下左段，
  完全覆盖的消失，横跨的裂成两段。返回升序元组列表。
- `total_length(ranges)`：先 `merge` 去重，再对每段累加 `hi - lo + 1`；空输入返回 0。

边界：`merge([[1,2],[4,5]])` 保留两段（缺 1 个整数），`gap=1` 时合并；
`subtract([[0,4]],(4,9)) == [(0,3)]`（端点在洞内会切掉）；
`total_length([[0,2],[2,4]]) == 5`（端点重叠只算一次）。

## 6. flags (`flags.py`)

`Impl.parse(argv)` 返回 `{"values": {key: [..]}, "flags": {key: True}, "positional": [..]}`：
- 不以 `-` 开头或正好是 `-` 的项，按顺序进 positional。
- 遇到 `--` 后所有项（含后续 `--`）都进 positional，`--` 本身不入结果。
- `--key=value` → `values[key].append(value)`；`--key value`（下一项不以 `--` 开头）同样。
- `--key` 到结尾或下一项以 `--` 开头 → `flags[key] = True`。
- `-abc` → 依次置 `flags` 的 a、b、c 为 True。
- values 每个值为列表，重复键按出现顺序累积；重复开关保持 `True`。

边界：`--k -5` 把 `-5` 当作值（只有 `--` 前缀的下一项才会阻止取值）；
`--x y --x` → values `{x:["y"]}` 且 flags `{x:True}`；空输入返回空结构。

## 复现的额外校验（非测试文件，临时脚本）

用一次性脚本核对了引号解析、非法金额报错、all+any 组合、其余账户负金额报错、
端点相接不交叠、subtract 部分覆盖、`total_length` 重叠去重、`--` 与短开关等边界，
输出与上述语义一致。该脚本未写入工作区。
