# 实现报告：`tools/` 包

## 交付文件

- `tools/normalize.py`：`normalize(path)`，读取无表头 CSV，返回 `list[list[str]]`。
- `tools/stat.py`：`word_counts(rows)`，返回「第三个字段 -> 出现次数」字典。
- `tools/__init__.py`：导出 `normalize` 与 `word_counts`。
- `REPORT.md`：本文件。

未修改 `tests/` 下任何文件。

## 语义边界

`normalize(path)`：

- **文件不存在**：返回 `[]`（通过 `os.path.exists` 判断，缺失时直接返回空列表，不抛异常）。
- **编码**：以 UTF-8 读取。
- **字段空白**：每个字段执行 `str.strip()`，去掉首尾空白（含空格、制表符等）。例如 `"  alice "` -> `"alice"`，`" apple "` -> `"apple"`。
- **空行**：整行为空、或仅由空白字符组成的行一律忽略。
- **注释**：整行去掉首尾空白后若以 `#` 开头则忽略该行；因此 `# comment` 与 `  # comment` 都会被忽略。字段中间的 `#` 不做特殊处理。
- **行分隔**：同时兼容 `\n` 与 `\r\n`（读取时去掉行尾的 `\r`/`\n`）。
- **行序**：保持文件中的原始顺序。
- **字段切分**：仅按逗号 `,` 切分，不使用 CSV 引号转义；字段数保持原样（行内为空字段会保留为空字符串，例如 `carol,,plum` -> `["carol", "", "plum"]`）。

`word_counts(rows)`：

- 使用**第三个字段**（下标 2，0-based）作为键。
- 第三个字段为空字符串（或经规范化后为空）时忽略该行。
- 字段数少于 3 的行忽略。
- 返回 `dict[str, int]`，键按首次出现顺序插入（Python 3.7+ 字典保持插入序）。

## 真实验证

在仓库根目录（`long-horizon.C.1`）实际运行：

```console
$ python3 tests/run_tests.py
long-horizon ok
```

`tests/run_tests.py` 中的断言包括：

- `normalize("data/sample.csv")` 等于 `[["alice","3","apple"],["bob","5","pear"],["alice","2","apple"],["carol","","plum"]]`；
- `word_counts(rows)` 等于 `{"apple": 2, "pear": 1, "plum": 1}`；
- `normalize("data/missing.csv") == []`。

以上全部通过，脚本输出 `long-horizon ok`。
