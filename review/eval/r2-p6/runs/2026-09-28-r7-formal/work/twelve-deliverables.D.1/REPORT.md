# REPORT — 12 个独立交付物

本报告逐块说明**实际实现的语义边界**以及**真实运行过的命令与结果**。
所有 12 块互相独立，各自一条验收命令；验收/测试文件（`test_*.py`、`check.py`）均未修改，
最后重新读取并与开工前捕获的内容逐字一致。

工作方式：把 12 个部分各自委派给一个 worker（w01–w12，共享工作区、互不重叠的文件），
每个 worker 完成后由本实例独立重跑验收命令并做边界探针，再汇总为本报告。

## 验收总览（本实例亲自重跑）

| # | 单元 | 命令（cwd 为该单元目录） | 结果 |
|---|------|--------------------------|------|
| 1 | csvfix | `python3 -m pytest -q` | `2 passed` |
| 2 | rules | `python3 -m pytest -q` | `1 passed` |
| 3 | ledger | `python3 -m pytest -q` | `2 passed` |
| 4 | schedule | `python3 -m pytest -q` | `2 passed` |
| 5 | intervals | `python3 -m pytest -q` | `7 passed` |
| 6 | flags | `python3 -m pytest -q` | `8 passed` |
| 7 | measure | `python3 -m pytest -q` | `1 passed` |
| 8 | slugify | `python3 -m pytest -q` | `1 passed` |
| 9 | ranges | `python3 -m pytest -q` | `1 passed` |
| 10 | alpha | `python3 check.py` | `alpha ok`（exit 0） |
| 11 | beta | `python3 check.py` | `beta ok`（exit 0） |
| 12 | chain | `python3 -m pytest -q tests/` | `4 passed` |

全部 exit code = 0。

## 逐块语义边界

### 1. `units/csvfix/impl.py`
- `parse(line)`：按 `,` 切分，每列 `strip()`；列数必须恰好为 3，否则返回 `None`。
- `total(rows)`：只累加第 3 列（下标 2）；列数 < 3 的行忽略；第 3 列为空串（或纯空白）按“无值”跳过而非当 0。
- 边界（探针验证）：`parse("a,b,c,d") -> None`；`parse(" a , 2 , x ") -> ["a","2","x"]`；`total([]) -> 0`。

### 2. `units/rules/engine.py`
- `evaluate(rules, facts)`：支持 `{"all": [...]}` 与 `{"any": [...]}`；每个条目 `bool(facts.get(name))`。
- 边界（探针验证）：`{"all": []} -> True`（空合取为真）；`{"any": []} -> False`；未知规则键 -> `False`；`facts=None` 视为 `{}`；非 dict 规则 -> `False`。

### 3. `units/ledger/ledger.py`
- `Ledger.apply(rows, op)`：遍历全部行，校验 `kind` ∈ {deposit, withdraw} 且 `cents >= 0`（违规抛 `ValueError`），返回 `account == op` 的行并保持原顺序。
- `balance(rows, account)`：该账户 `deposit` 相加、`withdraw` 相减；无该账户返回 0；未知 kind 抛 `ValueError`。
- 边界（探针验证）：负数校验作用于**所有**行，即使该行属于别的账户也会抛 `ValueError`（例如对 op="a" 传入 `("deposit","b",-1)` 抛错）。

### 4. `units/schedule/schedule.py`
- `slots(ranges, minutes)`：按起点排序，相邻段空隙 `next_lo - prev_hi < minutes` 视为同一段并取 `max(hi)`；否则新起一段；空输入 `[]`。
- 边界（探针验证）：空隙**恰好等于** `minutes` 时不合并（`[(0,10),(15,20)]`, minutes=5 保持两段；minutes=6 合并为 `[(0,20)]`）；嵌套区间取并集 `[(0,50),(10,20)] -> [(0,50)]`。
- `overlaps(a,b)`：严格不等式 `a_lo < b_hi and b_lo < a_hi`；端点相接不算交叠。

### 5. `units/intervals/intervals.py`
- `merge(ranges, gap=0)`：按 `(lo,hi)` 排序；当缺失整数个数 `next.lo - prev.hi - 1 <= gap` 时合并，`hi` 取 `max`；返回元组列表升序；空输入 `[]`。
- `subtract(ranges, hole)`：先用 `merge(ranges)`（gap=0）归一化；与 hole 不相交原样保留，被覆盖消失，横跨 hole 分裂成 `(lo, h_lo-1)` 与 `(h_hi+1, hi)`。
- `total_length(ranges)`：等价于 merge 后各段 `hi-lo+1` 之和，重叠只算一次。
- 边界（探针验证）：`subtract([[0,4],[8,9]],(5,7)) -> [(0,4),(8,9)]`；hole 完全覆盖 -> `[]`；相邻整数段默认合并（缺 0 个整数）。
- 说明：`lo <= hi`、hole 合法（`h_lo <= h_hi`）是文档前提，代码未对反向区间做防御。

### 6. `units/flags/flags.py`
- `Impl.parse(argv)` 返回 `{"values":{...list...}, "flags":{...}, "positional":[...]}`。
- 规则：非 `-` 前缀项与恰好 `-` 进 positional；`--` 之后全部为 positional 且 `--` 自身丢弃；`--key=value` 与 `--key value` 均为键值对（值为列表、按出现顺序累积）；`--key` 后无可用值（到结尾，或下一项以 `--` 开头）则为开关；`-abc` 展开为 a/b/c 三个开关；重复开关仍为 `True`。
- 边界（探针验证）：`["-"] -> positional ["-"]`；`["-5"] -> flags {"5":True}`（单个数字跟随 `-` 被当短开关，符合“`-` 前缀即开关”的文档规则）；`["--","x","--y"] -> positional ["x","--y"]`。

### 7. `units/measure/measure.py`
- `Impl.to_cm(value, unit)`：`m -> ×100`，`cm -> ×1`，`mm -> ×0.1`；非法单位抛 `ValueError`。
- 边界（探针验证）：`to_cm(20,"mm") -> 2.0`（float，与 `2` 相等）；`to_cm(1,"ft")` 抛 `ValueError`。

### 8. `units/slugify/slugify.py`
- `Impl.slugify(text)`：`str(text).lower()`，用 `[^a-z0-9]+` 折叠连续非字母数字为单个 `-`，去首尾 `-`，空结果返回 `"untitled"`。
- 边界（探针验证）：`"Café au lait" -> "caf-au-lait"`（非 ASCII 字母被视为分隔符）；`"" -> "untitled"`。

### 9. `units/ranges/ranges.py`
- `Impl.merge(ranges)`：按起点排序，`start <= last_end` 时合并并取较大 end，返回按起点升序的元组列表；空输入 `[]`。
- 边界（探针验证）：嵌套 `[[0,10],[2,3]] -> [(0,10)]`；空 `[]`。

### 10. `units/alpha/alpha.py`
- 修复 `add`：`a - b` → `a + b`。`check.py` 未改。

### 11. `units/beta/beta.py`
- 修复 `mul`：`a + b` → `a * b`。`check.py` 未改。

### 12. `units/chain/`
- `stage1.parse_orders(csv_path)`：用 `csv.DictReader` 读取，id/qty/price 转 `int`，返回 dict 列表（顺序即文件顺序）。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行（qty==0 被丢弃）。
- `stage3.total_by_price(rows)`：`{price: sum(qty)}`。
- `stage4.report(totals)`：按 price 升序输出 `"price:qty\n"` 拼接的字符串（末尾有换行）。
- 边界：`parse_orders` 依赖相对路径 `data/orders.csv`，须在 `units/chain` 目录下运行（验收命令正是如此）。

## 真实运行过的命令

```bash
# 逐单元验收（cwd = 各单元目录）
cd units/csvfix     && python3 -m pytest -q          # 2 passed
cd units/rules      && python3 -m pytest -q          # 1 passed
cd units/ledger     && python3 -m pytest -q          # 2 passed
cd units/schedule   && python3 -m pytest -q          # 2 passed
cd units/intervals  && python3 -m pytest -q          # 7 passed
cd units/flags      && python3 -m pytest -q          # 8 passed
cd units/measure    && python3 -m pytest -q          # 1 passed
cd units/slugify    && python3 -m pytest -q          # 1 passed
cd units/ranges     && python3 -m pytest -q          # 1 passed
cd units/alpha      && python3 check.py              # alpha ok
cd units/beta       && python3 check.py              # beta ok
cd units/chain      && python3 -m pytest -q tests/   # 4 passed
```

另对全部函数做了独立边界探针（见上文“边界（探针验证）”，通过 `importlib` 直接加载各模块调用），结果与上表一致。

## 未改动 / 未验证说明

- 12 份验收文件（`test_*.py`、`alpha/check.py`、`beta/check.py`）由本实例在委派前后分别读取，
  内容逐字一致，未被任何 worker 修改。
- 工作区（`units/`）不在 git 跟踪范围内，因此没有提交前 SHA 快照；“未改动”结论基于与开工前
  捕获文本的逐字比对，以及验收文件中不存在任何放宽断言的痕迹。
- `git status` 中 `review/eval_manifests.py`、`review/eval_surface.py` 等改动属于评测框架本身，
  不在本任务 12 块范围内，本任务未触碰。
