工作区里有两个**彼此独立**的交付物，可以分别完成（也可以并行）：

1. `csvfix/`：`impl.py` 的 `parse`/`total` 有缺陷，`test_csvfix.py` 表达了期望语义
   （`parse` 裁剪每个字段、列数不是 3 返回 `None`；`total` 跳过空金额字段并把第三列当金额）。
   修好实现使 `cd csvfix && python3 -m pytest -q` 通过。**不要修改测试文件**。
2. `rules/`：实现 `engine.evaluate(rules, facts)`，支持 `{"all": [...]}` 与 `{"any": [...]}`，
   使 `cd rules && python3 -m pytest -q` 通过。**不要修改测试文件**。
3. 写 `INDEX.md`，用几行说明两个交付物各自是什么、你是怎么验证的（含真实命令与结果）。

完成后用一段简短说明报告：改了哪些文件、真实运行过哪些命令、结果如何。不要声称没有真正运行过的命令或结果。
