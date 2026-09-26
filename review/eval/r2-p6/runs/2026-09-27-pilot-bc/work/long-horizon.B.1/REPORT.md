# REPORT

## 实现内容

- `tools/__init__.py`：导出 `normalize` 与 `word_counts`。
- `tools/normalize.py`：`normalize(path)` 读取无表头 CSV。
- `tools/stat.py`：`word_counts(rows)` 统计第三个字段出现次数。

## 语义边界

`normalize(path)`：

- 以 UTF-8 文本方式打开文件；按行处理，返回 `list[list[str]]`。
- **字段空白**：对每个字段做首尾空白去除（`str.strip()`），例如 `  alice ` -> `alice`。
- **空行**：完全跳过（包括只含空白字符的行）。
- **整行注释**：该行去除首尾空白后以 `#` 开头的整行被忽略。因此 `# comment` 和 `  # indented` 都会被忽略；`a,#b,c` 这类行内 `#` **不**视作注释（只有行首才是）。
- **行序**：保持文件中的原始顺序。
- **缺失字段**：不做补齐。字段数少于 3 的行（如 `x,y`）按原样保留为 `["x", "y"]`；字段数为 3 但第三个为空（如 `p,q,`）保留为 `["p", "q", ""]`。
- **文件不存在**：返回 `[]`。

`word_counts(rows)`：

- 只统计每行的第三个字段（下标 `2`）。
- 第三个字段为空字符串时忽略该行。
- 行长度不足 3（缺少第三个字段）时同样忽略该行。
- 返回 `dict[str, int]`，键为字段值，值为出现次数；按首次出现顺序插入（Python 3.7+ dict 保序）。
- 输入中的字段值假定已由 `normalize` 去空白；`word_counts` 本身不再做 strip。

## 真实运行过的命令与结果

1. `python3 tests/run_tests.py`
   - 输出：`long-horizon ok`
   - 退出码 0，全部断言通过。

2. 边界用例脚本（临时文件，含空行、缩进注释、缺列、空第三字段）：

   ```bash
   python3 - <<'PY'
   from tools import normalize, word_counts
   import tempfile, pathlib
   rows = normalize("data/sample.csv")
   print("sample rows:", rows)
   print("counts:", word_counts(rows))
   print("missing file:", normalize("data/missing.csv"))
   tmp = pathlib.Path(tempfile.mkdtemp()) / "edge.csv"
   tmp.write_text("\n   \n# full comment\n  # indented comment\na, b ,c\nx,y\np,q,\n", encoding="utf-8")
   r = normalize(tmp)
   print("edge rows:", r)
   print("edge counts:", word_counts(r))
   PY
   ```

   实际输出：

   ```
   sample rows: [['alice', '3', 'apple'], ['bob', '5', 'pear'], ['alice', '2', 'apple'], ['carol', '', 'plum']]
   counts: {'apple': 2, 'pear': 1, 'plum': 1}
   missing file: []
   edge rows: [['a', 'b', 'c'], ['x', 'y'], ['p', 'q', '']]
   edge counts: {'c': 1}
   ```

## 未修改

未改动 `tests/` 下的任何文件，也未改动 `data/`。
