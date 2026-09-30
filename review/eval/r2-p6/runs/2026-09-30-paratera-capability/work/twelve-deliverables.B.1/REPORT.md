# REPORT — 12 个独立交付物

工作区：`twelve-deliverables.B.1`。所有实现均未改动测试/验收文件（`test_*.py`、`check.py`、`tests/`）。

## 逐块说明

### 1. `units/csvfix/impl.py` — 修好 `parse`/`total`
- `parse(line)`：用标准 CSV 语义切分（支持引号，如 `"a,b"` 视为一个字段），每列 `strip()`；
  字段数不等于 3 返回 `None`。
- `total(rows)`：每行按 `[名称, 数量, 单价]` 解释，累加 `数量 * 单价`（文档字符串指出
  “把第三列当成金额”是 bug，故金额取数量×单价）。任一字段为空白则跳过该行；非整数抛 `ValueError`。
  边界：行长度 < 3 也抛 `ValueError`。
- 注：给定测试对 `[["a","1","2"],["b","2",""]]` 期望 2，数量×单价与“仅第三列”都得 2，
  测试本身不能区分；此处依据文档字符串选择乘积语义。

### 2. `units/rules/engine.py` — `evaluate(rules, facts)`
- `{"all": [...]}` → 所有键在 facts 中为真；`{"any": [...]}` → 任意键为真；
  缺失键按 `False`；空/未知规则返回 `False`；返回真正的 `bool`。

### 3. `units/ledger/ledger.py` — `Ledger.apply` / `balance`
- `apply(rows, op)`：先校验所有行金额非负（负数抛 `ValueError`），再返回 account == op 的行，
  保持原顺序。`op` 即账户名。
- `balance(rows, account)`：`deposit` 加、`withdraw` 减，忽略其他账户；未知 kind 抛 `ValueError`；
  无匹配返回 0。

### 4. `units/schedule/schedule.py` — `slots` / `overlaps`
- `slots(ranges, minutes)`：按 lo 排序，`next_lo - prev_hi < minutes` 视为同段合并（取 hi 最大值）；
  否则新段。空输入返回 `[]`，结果为 `(lo, hi)` 元组升序列表。
- `overlaps(a, b)`：`max(lo) < min(hi)` 才算交叠，端点相接不算。

### 5. `units/intervals/intervals.py` — `Impl.merge`/`subtract`/`total_length`
- `merge(ranges, gap=0)`：排序后按“缺失整数个数 = next.lo - prev.hi - 1 <= gap”合并；空返回 `[]`。
- `subtract(ranges, hole)`：先用默认 gap 归一化输入，再逐段与 hole 求差；不相交原样保留，
  完全覆盖消失，跨越则分裂成 `(lo, hlo-1)` 与 `(hhi+1, hi)`。
- `total_length(ranges)`：合并后累加 `hi - lo + 1`，重叠只算一次。

### 6. `units/flags/flags.py` — `Impl.parse`
- 不以 `-` 开头或正好 `-` → positional；`--` 终止解析，其后全部 positional。
- `--key=value`、`--key value`（值不以 `--` 开头才作为值）→ `values[key].append`。
- 无可用值的 `--key` → `flags[key]=True`；`-abc` → flags a/b/c。
- 开关重复仍为 True；键值与开关同名可并存。

### 7. `units/measure/measure.py` — `Impl.to_cm(value, unit)`
- `m`×100、`cm` 原值、`mm`÷10；其它单位抛 `ValueError`。

### 8. `units/slugify/slugify.py` — `Impl.slugify(text)`
- 逐字符：字母数字转小写保留；连续非字母数字折叠成一个 `-`；去首尾 `-`；空结果返回 `untitled`。
  （用 `str.isalnum()`，因此也保留 Unicode 字母数字。）

### 9. `units/ranges/ranges.py` — `Impl.merge(ranges)`
- 排序后，`next.lo <= cur.hi` 即合并（只合并有交叠的闭区间，相邻不交叠），输出 `(lo, hi)` 元组升序。

### 10. `units/alpha/alpha.py`
- `add` 由 `a - b` 改为 `a + b`。

### 11. `units/beta/beta.py`
- `mul` 由 `a + b` 改为 `a * b`。

### 12. `units/chain/` — `stage1`…`stage4`
- `stage1.parse_orders(path)`：`csv.DictReader` 读 `id,qty,price`，转 `int`，返回 dict 列表。
- `stage2.filter_orders(rows)`：保留 `qty > 0`。
- `stage3.total_by_price(rows)`：按 `price` 分组累加 `qty`。
- `stage4.report(totals)`：按 price 升序渲染 `"price:qty\n"` 拼接。

## 真实运行过的命令与结果

从工作区根目录 `WS=.../twelve-deliverables.B.1` 执行：

- 基线（修改前）：`units/csvfix` → `1 failed, 1 passed`；`units/alpha`、`units/beta` 的
  `python3 check.py` 均 `AssertionError`（add 得 -1、mul 得 7）。

修改后：

```
units/csvfix   python3 -m pytest -q        -> 2 passed
units/rules    python3 -m pytest -q        -> 1 passed
units/ledger   python3 -m pytest -q        -> 2 passed
units/schedule python3 -m pytest -q        -> 2 passed
units/intervals python3 -m pytest -q       -> 7 passed
units/flags    python3 -m pytest -q        -> 8 passed
units/measure  python3 -m pytest -q        -> 1 passed
units/slugify  python3 -m pytest -q        -> 1 passed
units/ranges   python3 -m pytest -q        -> 1 passed
units/alpha    python3 check.py            -> "alpha ok", exit=0
units/beta     python3 check.py            -> "beta ok", exit=0
units/chain    python3 -m pytest -q tests/ -> 4 passed
```

额外的边界抽查（`python3 -` 内联脚本）输出：

```
csvfix quote: ['a,b', '2', 'x'] | short: None
csvfix total product: 12 | bad raises: yes
flags terminator: {'values': {}, 'flags': {}, 'positional': ['--x', 'y']}
flags dash value: {'values': {'k': ['-5']}, 'flags': {}, 'positional': []} | eq: {'values': {'k': ['--v']}, 'flags': {}, 'positional': []}
flags mixed: {'values': {'x': ['y']}, 'flags': {'x': True}, 'positional': []}
intervals: [(0, 12)] [(1, 1), (7, 7)] 5
schedule: [(0, 90), (120, 150)] False
slugify: hello-world a-b untitled
```

## 未改动

`units/*/test_*.py`、`units/*/check.py`、`units/chain/tests/*` 全部保持原样。
