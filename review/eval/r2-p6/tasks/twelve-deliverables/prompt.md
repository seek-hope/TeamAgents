工作区里有 12 个**互相独立**的交付物，各自的验收已经冻结（**不要修改任何测试或验收文件**）：

1. `units/csvfix/`：修好 `impl.py` 的 `parse`/`total`，`cd units/csvfix && python3 -m pytest -q` 通过。
2. `units/rules/`：实现 `engine.evaluate(rules, facts)`（支持 `{"all": [...]}` 与 `{"any": [...]}`）。
3. `units/ledger/`：实现 `Ledger.apply` 与 `balance`。
4. `units/schedule/`：实现 `slots` 与 `overlaps`。
5. `units/intervals/`：实现 `Impl` 的 `merge`/`subtract`/`total_length`。
6. `units/flags/`：实现 `Impl.parse`。
7. `units/measure/`：实现 `Impl.to_cm(value, unit)`（`m`/`cm`/`mm`，非法单位抛 `ValueError`）。
8. `units/slugify/`：实现 `Impl.slugify(text)`（小写化、折叠非字母数字、去首尾 `-`、空结果 `untitled`）。
9. `units/ranges/`：实现 `Impl.merge(ranges)`（合并闭区间，返回按起点排序的元组列表）。
10. `units/alpha/`：修好 `alpha.py`，使 `cd units/alpha && python3 check.py` 通过（不要改 `check.py`）。
11. `units/beta/`：修好 `beta.py`，使 `cd units/beta && python3 check.py` 通过（不要改 `check.py`）。
12. `units/chain/`：实现 `stage1` 到 `stage4`，使 `cd units/chain && python3 -m pytest -q tests/` 通过。

每个目录里的文档字符串与测试文件说明了该单元的语义；12 块之间没有依赖，可以各自独立完成。
最后写 `REPORT.md`：逐块说明你实现的语义边界以及你真实运行过的命令与结果。

完成后用一段简短说明报告：改了哪些文件、真实跑过哪些命令、结果如何。不要声称没有真正运行过的命令或结果。
