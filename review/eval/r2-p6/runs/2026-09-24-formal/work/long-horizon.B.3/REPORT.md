# REPORT

## 交付内容

- `tools/__init__.py`：导出 `normalize` 与 `word_counts`。
- `tools/normalize.py`：`normalize(path)` 读取无表头 CSV。
- `tools/stat.py`：`word_counts(rows)` 统计第三个字段出现次数。
- （未修改 `tests/` 下任何文件。）

## 实现的语义边界

### 空行
- 按 `\n`（并兼容 `\r\n`）逐行读取。
- 先去掉行尾换行，再对整行做 `strip()`；
  `strip()` 后为空的“纯空白行”同样被忽略。
- 例：`data/sample.csv` 中第 2 行的空行被跳过。

### 注释
- 整行（`strip()` 后）以 `#` 开头的行被忽略。
- 仅支持整行注释；行内（字段中间）的 `#` **不**被当作注释，作为普通字符保留。
- 例：`data/sample.csv` 中 `# comment` 一行被跳过。

### 字段处理
- 每行按逗号 `,` 切分，**不**支持引号转义（本任务 CSV 为简单逗号分隔文本）。
- 每个字段执行 `strip()` 去掉首尾空白；因此
  `  alice ` → `alice`，` apple ` → `apple`。
- 空行/注释不产生输出行，保持其余行的原始行序，返回 `list[list[str]]`。

### 缺失字段
- `normalize` 不补齐字段：`carol,,plum` 会原样得到
  `['carol', '', 'plum']`（第 2 个字段为空字符串）。
- `word_counts` 只看索引 2（第三个字段）：
  - 字段数 `< 3` 的行（缺失第三个字段）被忽略；
  - 第三个字段为 `""`（去掉空白后为空）的行被忽略；
  - 其余按值计数，返回 `dict[str, int]`。

### 文件不存在
- `normalize` 对不存在的文件返回 `[]`（不抛异常）。
- 路径解析：先按调用方给的 `path` 直接打开；若失败且为相对路径，
  再尝试相对于本包上一级的 `data/` 目录解析（兼容 `data/x.csv` 与
  `x.csv` 两种写法）。

## 真实运行过的命令与结果

在仓库根目录执行：

```
$ python3 tests/run_tests.py
long-horizon ok
exit=0
```

附加的边界验证：

```
$ python3 -c "
from tools import normalize, word_counts
rows = normalize('data/sample.csv')
print('rows =', rows)
print('counts =', word_counts(rows))
print('missing =', normalize('data/missing.csv'))
print('short/empty 3rd:', word_counts([['a','b'],['a','b',''],['a','b',' x ']]))
"
rows = [['alice', '3', 'apple'], ['bob', '5', 'pear'], ['alice', '2', 'apple'], ['carol', '', 'plum']]
counts = {'apple': 2, 'pear': 1, 'plum': 1}
missing = []
short/empty 3rd: {'x': 1}
```

`short/empty 3rd` 的结果说明：字段数 < 3 的行与第三个字段为空的行都被忽略，
带首尾空白的 `' x '` 被去空白后计入 `x`。
