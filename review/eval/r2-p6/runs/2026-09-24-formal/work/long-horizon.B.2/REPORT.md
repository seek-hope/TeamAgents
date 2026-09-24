# REPORT

## 交付内容

- `tools/__init__.py`：导出 `normalize` 与 `word_counts`。
- `tools/normalize.py`：实现 `normalize(path)`。
- `tools/stat.py`：实现 `word_counts(rows)`。

未修改 `tests/` 与 `data/` 下的任何文件。

## 语义边界

### `normalize(path)`

- 按 UTF-8 文本打开，无表头，逗号分隔。
- 每个字段使用 `str.strip()` 去掉首尾空白（空格、制表符等）。
- 空行（整行 strip 后为空）被忽略。
- 以 `#` 开头的整行视为注释并忽略；由于先对整行 strip，`   # comment` 同样被忽略。
- 行序保持不变。
- 字段拆分使用 `line.split(",")`，因此字段内部不处理引号转义（数据格式约定为简单 CSV）。
- 文件不存在时返回 `[]`（用 `os.path.isfile` 判断，目录路径也返回 `[]`）。

### `word_counts(rows)`

- 返回“第三个字段（下标 2）-> 出现次数”的字典。
- 第三个字段为空字符串时忽略。
- 行字段数少于 3 时视为缺失第三个字段，忽略该行。

## 真实运行过的命令与结果

```
$ python3 tests/run_tests.py
long-horizon ok
exit=0
```

测试脚本的全部断言通过，包括：

- `normalize("data/sample.csv")` 返回
  `[["alice","3","apple"], ["bob","5","pear"], ["alice","2","apple"], ["carol","","plum"]]`；
- `word_counts(rows)` 返回 `{"apple": 2, "pear": 1, "plum": 1}`；
- `normalize("data/missing.csv") == []`。
