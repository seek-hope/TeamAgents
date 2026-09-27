# 12 个独立交付物 —— 实现报告

## 方式与团队

12 个交付物互相独立，因此为每一块各派一个 worker 实例（共 12 个），每块任务都带自己的验收命令；
worker 只改各自的实现文件，禁止改任何测试 / 验收文件。随后由我（负责人）在集成阶段独立重跑全部 12 条验收命令。
实现文件与验收/测试文件的职责严格分离。

改动文件（仅实现文件）：

- `units/csvfix/impl.py`
- `units/rules/engine.py`
- `units/ledger/ledger.py`
- `units/schedule/schedule.py`
- `units/intervals/intervals.py`
- `units/flags/flags.py`
- `units/measure/measure.py`
- `units/slugify/slugify.py`
- `units/ranges/ranges.py`
- `units/alpha/alpha.py`
- `units/beta/beta.py`
- `units/chain/stage1.py`, `stage2.py`, `stage3.py`, `stage4.py`

未改动的冻结文件（mtime 仍为 9/24，而实现文件为 9/28 06:03 之后）：
`units/*/test_*.py`、`units/alpha/check.py`、`units/beta/check.py`、`units/chain/tests/*.py`、`units/chain/data/orders.csv`。

---

## 1. units/csvfix —— `impl.py` 的 `parse` / `total`

语义边界：

- `parse(line)`：用 `csv` 模块切分，**支持引号字段**（引号内的逗号不被当作分隔符）；对每个字段做
  `strip()`；字段数不等于 3 时返回 `None`。
- `total(rows)`：第三列是金额；空字符串金额按 0 计；`None` 或不足 3 列的行走跳过；非空且非数字的金额
  抛 `ValueError`；返回整数和。
- 边界：引号闭合/转义遵循标准 `csv` 语义；“恰好 3 列”是对引号解析后的字段计数。

真实运行的命令与结果：

```
$ cd units/csvfix && python3 -c "import impl; assert impl.parse(' a , 2 , x ') == ['a','2','x']; assert impl.parse('a,2') is None; assert impl.parse('\"a,b\", 2 , x') == ['a,b','2','x']; assert impl.parse('\"x,y\",2') is None; assert impl.total([['a','1','2'],['b','2','']]) == 2; print('quotes+trim+validate+total ok')"
quotes+trim+validate+total ok
$ cd units/csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.05s
```

## 2. units/rules —— `engine.evaluate`

语义边界：`{"all": [...]}` 当所有名字对应的事实为真时返回 `True`；`{"any": [...]}` 任一为真即 `True`；
缺失的事实名视为假；返回真正的 `bool`；两个键都没有时抛 `ValueError`。边界：两个键同时存在时优先按
`all` 处理；空列表时 `all` 为 `True`、`any` 为 `False`。

```
$ cd units/rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

## 3. units/ledger —— `Ledger.apply` / `balance`

语义边界：`apply(rows, op)` 先对**所有**行做校验（任一行金额为负即抛 `ValueError`），再返回账号等于
`op` 的行并保持原顺序；`balance(rows, account)` 对 deposit 相加、withdraw 相减，其他账号忽略，
没有该账号时返回 0。边界：识别不了的 kind 不计入；校验是全局的（别的账号出现负数也会抛错）。

```
$ cd units/ledger && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.01s
```

## 4. units/schedule —— `slots` / `overlaps`

语义边界：`slots(ranges, minutes)` 按起点排序，当 `next_lo - prev_hi < minutes`（严格小于）时合并，
返回按起点升序的区间；空输入返回 `[]`。`overlaps(a, b)` 用严格不等式 `a_lo < b_hi and b_lo < a_hi`，
端点相接不算重叠。边界：`minutes=0` 时相邻（gap=0）不合并、只有真正重叠才合并；零长度区间永不重叠。

```
$ cd units/schedule && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.02s
```

## 5. units/intervals —— `Impl.merge` / `subtract` / `total_length`

语义边界：闭整数区间，假定 `lo <= hi`。`merge(ranges, gap=0)` 按 `(lo, hi)` 排序，相邻两段缺失整数个数
`next.lo - prev.hi - 1 <= gap` 时合并（嵌套取较大 hi）。`subtract(ranges, hole)` 先用 merge 归一化，
不相交段保留、完全覆盖段消失、跨越段在 `hole_lo-1` / `hole_hi+1` 处分裂。`total_length` 先 merge 再累加
`hi-lo+1`，重叠只算一次。

```
$ cd units/intervals && python3 -m pytest -q
.......                                                                  [100%]
7 passed in 0.02s
```

## 6. units/flags —— `Impl.parse`

语义边界：输出 `{"values": {...}, "flags": {...}, "positional": [...]}`。不以 `-` 开头或正好是 `-`
的项按序进 positional；`--` 之后全部进 positional（`--` 本身不出现）；`--key=value` 与 `--key value`
都是键值对；`--key` 无可用值（到结尾或下一项以 `--` 开头）时是开关；`-abc` 拆成开关 a、b、c；
values 是列表、重复键按序累积；重复开关仍为 `True`。
边界：`--key` 后面的值即使以单个 `-` 开头（如 `-5`）也算值，只有 `--` 前缀才判为开关；
`--key=` 得到空串值；`--key=a=b` 的值为 `a=b`；不支持“短选项带值”（规范未要求）。

```
$ cd units/flags && python3 -m pytest -q
........                                                                 [100%]
8 passed in 0.02s
```

## 7. units/measure —— `Impl.to_cm`

语义边界：m→×100、cm→×1、mm→×0.1；其余单位抛 `ValueError`。边界：单位区分大小写；非数值
value 的失败方式未定义（会由乘法抛 `TypeError`）。

```
$ cd units/measure && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

## 8. units/slugify —— `Impl.slugify`

语义边界：转小写；把每段非字母数字字符折叠为单个 `-`；去掉首尾 `-`；结果为空返回 `untitled`。
边界：使用 Unicode 感知的 `isalnum()`（带音标字母会被保留）；`None` 先转为空串，最终返回 `untitled`。

```
$ cd units/slugify && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.01s
```

## 9. units/ranges —— `Impl.merge`

语义边界：每个输入是 `[start, end]` 闭区间；先 `min/max` 归一化（倒序也支持），按起点排序，
当 `start <= 上一段.end` 时合并（嵌套取较大 end），返回按起点排序的元组列表。
边界：**相邻但不重叠**（如 `[1,2]` 与 `[3,4]`）不合并——与验收中 `[1,4]`、`[5,7]` 保持分离一致。

```
$ cd units/ranges && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.02s
```

## 10. units/alpha —— `alpha.add`

语义边界：`add(a, b)` 返回 `a + b`（原为 `a - b`）。

```
$ cd units/alpha && python3 check.py
alpha ok
```

## 11. units/beta —— `beta.mul`

语义边界：`mul(a, b)` 返回 `a * b`（原为 `a + b`）。

```
$ cd units/beta && python3 check.py
beta ok
```

## 12. units/chain —— `stage1` ~ `stage4`

语义边界：

- `stage1.parse_orders(path)`：用 `csv.DictReader` 读取 `id,qty,price`，跳过表头，`id/qty/price` 转 int，
  返回 dict 列表。路径相对当前工作目录。
- `stage2.filter_orders(rows)`：保留 `qty > 0` 的行，顺序不变。
- `stage3.total_by_price(rows)`：按 price 汇总 qty，返回 `{price: total}`。
- `stage4.report(totals)`：按 price 升序渲染 `"price:total\n"` 串联（最后一行以换行结束）。

```
$ cd units/chain && python3 -m pytest -q tests/
....                                                                     [100%]
4 passed in 0.01s
```

---

## 集成验收（负责人独立重跑，全部 12 条）

```
$ for d in csvfix rules ledger schedule intervals flags measure slugify ranges; do (cd units/$d && python3 -m pytest -q); done
csvfix:    2 passed   rules:     1 passed   ledger:    2 passed   schedule:  2 passed
intervals: 7 passed   flags:     8 passed   measure:   1 passed   slugify:   1 passed
ranges:    1 passed
$ cd units/alpha && python3 check.py   -> alpha ok
$ cd units/beta  && python3 check.py   -> beta ok
$ cd units/chain && python3 -m pytest -q tests/  -> 4 passed
```

12/12 条验收命令退出码均为 0。
