"""3 列 CSV 的小工具。"""

import csv
import io


def parse(line):
    """把一行 CSV 解析成 3 个字段的列表。

    - 支持双引号包裹的字段：引号内的逗号不是分隔符，"" 表示一个字面引号。
    - 去掉每个字段前后的空白。
    - 列数不是 3 时返回 None。
    """
    try:
        fields = next(csv.reader(io.StringIO(line)))
    except Exception:
        return None
    fields = [field.strip() for field in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """rows 的每行形如 [label, qty, price]，返回 qty * price 之和。

    - 任一数值字段为空的行整行跳过（不把空字段当作 0）。
    - 非空但无法转成整数的数值字段会抛 ValueError。
    """
    total_sum = 0
    for row in rows:
        if len(row) < 3:
            continue
        qty_s, price_s = row[1], row[2]
        if qty_s == "" or price_s == "":
            continue
        total_sum += int(qty_s) * int(price_s)
    return total_sum
