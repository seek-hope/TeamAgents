工作区里有 6 个**互相独立**的交付物，各自的验收测试已经冻结（**不要修改任何测试文件**）：

1. `units/csvfix/`：修好 `impl.py` 的 `parse`/`total`，使 `cd units/csvfix && python3 -m pytest -q` 通过。
2. `units/rules/`：实现 `engine.evaluate(rules, facts)`，支持 `{"all": [...]}` 与 `{"any": [...]}`，
   使 `cd units/rules && python3 -m pytest -q` 通过。
3. `units/ledger/`：实现 `Ledger.apply` 与 `balance`，使 `cd units/ledger && python3 -m pytest -q` 通过。
4. `units/schedule/`：实现 `slots` 与 `overlaps`，使 `cd units/schedule && python3 -m pytest -q` 通过。
5. `units/intervals/`：实现 `Impl` 的 `merge`/`subtract`/`total_length`，
   使 `cd units/intervals && python3 -m pytest -q` 通过。
6. `units/flags/`：实现 `Impl.parse`，使 `cd units/flags && python3 -m pytest -q` 通过。

每个目录里都有说明该单元语义的文档字符串与测试文件，六块之间没有依赖，可以各自独立完成。
最后写 `REPORT.md`：逐块说明你实现的语义边界（边界情况怎么处理的）以及你真实运行过的命令与结果。

完成后用一段简短说明报告：改了哪些文件、真实跑过哪些命令、结果如何。不要声称没有真正运行过的命令或结果。
