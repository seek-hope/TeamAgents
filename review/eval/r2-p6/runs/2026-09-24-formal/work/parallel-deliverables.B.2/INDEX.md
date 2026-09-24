# INDEX

本工作区包含两个彼此独立的交付物。

## 1. `csvfix/` —— CSV 解析与求和修复
- 文件：`csvfix/impl.py`（实现）、`csvfix/test_csvfix.py`（测试，未改动）。
- `parse(line)`：按逗号拆分并逐个 `strip()`，列数不是 3 时返回 `None`。
- `total(rows)`：把每行第三列当金额求和，跳过空/纯空白的金额字段。

## 2. `rules/` —— 规则求值引擎
- 文件：`rules/engine.py`（实现）、`rules/test_rules.py`（测试，未改动）。
- `evaluate(rules, facts)`：支持 `{"all": [...]}`（全部为真）与 `{"any": [...]}`（任一为真）。

## 验证方式（真实运行的命令与结果）
```
$ cd csvfix && python3 -m pytest -q
..                                                                       [100%]
2 passed in 0.00s

$ cd rules && python3 -m pytest -q
.                                                                        [100%]
1 passed in 0.00s
```
两个测试文件均未修改。
