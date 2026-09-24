# INDEX

两个**彼此独立**的交付物。

## 1. `csvfix/` — CSV 解析与金额汇总

- `impl.py`
  - `parse(line)`：按逗号切分并 `strip()` 每个字段；列数不等于 3 时返回 `None`。
    （用 `csv.reader` 解析单行，因此带引号的字段也能正确处理。）
  - `total(rows)`：把**第三列**当金额；列数不足或金额为空/全空白时跳过该行；
    金额非空但非数字时抛出 `ValueError`（"报告坏数据"，而不是当作 0）。
- `test_csvfix.py` 未改动，是语义来源。

## 2. `rules/` — 极简规则引擎

- `engine.py`
  - `evaluate(rules, facts)` 支持 `{"all": [...]}`（全部为真才真）与
    `{"any": [...]}`（任一为真即真）。
  - 列表项可以是事实名（在 `facts` 中查找，缺失视为 `False`），也可以是嵌套规则
    dict，因此两种组合子可以嵌套。
- `test_rules.py` 未改动。

## 验证方式（真实运行）

```
cd csvfix && python3 -m pytest -q
# ..  [100%]
# 2 passed in 0.00s      (EXIT=0)

cd rules && python3 -m pytest -q
# .   [100%]
# 1 passed in 0.00s      (EXIT=0)
```

两个测试文件均保持原样未修改。
