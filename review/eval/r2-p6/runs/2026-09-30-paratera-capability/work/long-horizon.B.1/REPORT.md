# REPORT

## 交付物

- `tools/__init__.py`：导出 `normalize` 与 `word_counts`（`__all__` 同步声明）。
- `tools/normalize.py`：`normalize(path) -> list[list[str]]`。
- `tools/stat.py`：`word_counts(rows) -> dict[str, int]`。
- 本文件 `REPORT.md`。

`tests/` 目录未做任何修改。

## 实现的语义边界

### normalize(path)

- **文件不存在**：`Path(path).is_file()` 为假（文件不存在、路径是目录等）时返回 `[]`，不抛异常。
- **空行**：整行去除空白后为空的行被忽略。
- **注释**：整行去除首尾空白后以 `#` 开头的行被忽略（样例中的 `# comment` 位于行首）。
- **字段空白**：每个字段用 `str.strip()` 去掉首尾空白；因此 `"  alice "` -> `"alice"`、`" apple "` -> `"apple"`。
- **空字段**：字段本身为空时保留为空字符串 `""`（例如 `carol,,plum` 的第二个字段）。
- **行序与表头**：保持原始行序；数据无表头，不做表头剔除。
- **行尾符**：读取时去除 `\n` / `\r`，UTF-8 解码。

### word_counts(rows)

- 以每行第三个字段（下标 `2`）作为键统计出现次数。
- 第三个字段为空字符串 `""` 时忽略该行。
- 字段数不足 3 的行（缺失字段）跳过，不抛异常。
- 重复值累加计数。

## 真实运行过的命令与结果

1. `cat -A data/sample.csv` —— 查看样例数据，确认含 `  alice , 3 ,apple`、空行、`# comment`、`carol,,plum` 等。
2. `python3 tests/run_tests.py`
   - 输出：`long-horizon ok`
   - 退出码：`0`
3. 额外边界自检：
   `python3 - <<'PY' ...`（调用 `normalize` / `word_counts`）
   - `normalize("data/missing.csv")` -> `[]`
   - `normalize("data/sample.csv")` -> `[['alice','3','apple'],['bob','5','pear'],['alice','2','apple'],['carol','','plum']]`
   - `word_counts(...)` -> `{'apple': 2, 'pear': 1, 'plum': 1}`
   - 空第三字段被忽略、字段数不足的行被跳过，均未抛异常。
