请在当前工作区实现一个 `tools/` 包，使 `python3 tests/run_tests.py` 全部通过（**不要修改 tests/ 下的任何文件**）：

- `tools/normalize.py`：`normalize(path)` 读取 `data/` 下的无表头 CSV（逗号分隔 UTF-8 文本），
  每个字段去掉首尾空白；忽略空行与以 `#` 开头的整行注释；保持行序；返回 `list[list[str]]`。
  文件不存在时返回空列表。
- `tools/stat.py`：`word_counts(rows)` 返回“第三个字段 -> 出现次数”的字典（忽略第三个字段为空的字段）。
- `tools/__init__.py`：导出 `normalize` 与 `word_counts`。
- 再写 `REPORT.md`：说明你实现的语义边界（空行/注释/缺失字段的处理）与你真实运行过的命令及结果。

完成后用一段简短说明报告：写了哪些文件、真实跑过哪些命令、结果如何。不要声称没有真正运行过的命令或结果。
