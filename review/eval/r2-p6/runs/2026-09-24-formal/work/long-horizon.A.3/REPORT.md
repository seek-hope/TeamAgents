# REPORT

## 交付物

- `tools/__init__.py`：导出 `normalize` 与 `word_counts`。
- `tools/normalize.py`：实现 `normalize(path)`。
- `tools/stat.py`：实现 `word_counts(rows)`。
- `REPORT.md`：本文件。

## 实现的语义边界

`normalize(path)`：

| 情形 | 处理 |
| --- | --- |
| 文件不存在 | 返回 `[]`（捕获 `FileNotFoundError`） |
| 空行 / 纯空白行 | 忽略（整行 `strip()` 后为空即跳过） |
| 以 `#` 开头的整行注释 | 忽略（先 `strip()` 再去判断，因此行首空白后的 `#` 也算注释） |
| 字段首尾空白 | 每个字段单独 `strip()` |
| 行序 | 保持文件中的原始顺序 |
| 编码 | UTF-8 |
| 表头 | 无表头，所有非空非注释行都是数据行 |
| 引号 | 不做 CSV 引号解析，仅按逗号 `,` 切分（数据约定为纯文本） |

`word_counts(rows)`：

| 情形 | 处理 |
| --- | --- |
| 正常行 | 取下标 2 的字段作为 key，计数 +1 |
| 第三个字段为空字符串 | 忽略，不计入 |
| 行字段数不足 3（缺失字段） | 忽略，不计入 |
| 重复字段 | 累加计数 |

## 真实运行过的命令与结果

1. 验收脚本（工作目录为仓库根，脚本内使用相对路径 `data/sample.csv`）：

   ```console
   $ cd /home/rimuru/Projects/Code/for_fun/TeamAgents/review/eval/r2-p6/runs/2026-09-24-formal/work/long-horizon.A.3
   $ python3 tests/run_tests.py
   long-horizon ok
   ```

   结果：无断言失败，输出 `long-horizon ok`（退出码 0）。

2. 手工验证边界（缺失文件、空第三个字段、字段数不足）：

   ```console
   $ python3 -c "
   from tools import normalize, word_counts
   print(normalize('data/sample.csv'))
   print(word_counts(normalize('data/sample.csv')))
   print(normalize('data/missing.csv'))
   print(word_counts([['x','y'],['a','b',''],['a','b','z']]))
   "
   [['alice', '3', 'apple'], ['bob', '5', 'pear'], ['alice', '2', 'apple'], ['carol', '', 'plum']]
   {'apple': 2, 'pear': 1, 'plum': 1}
   []
   {'z': 1}
   ```

   结果：`data/missing.csv` 返回 `[]`；空第三字段与不足三列的行被忽略；`z` 计数为 1。均符合预期。
