"""固定 3 列的 CSV 行解析与小计。

约定（见测试）：
- 一行恰好 3 列，列内容两端空白被裁剪；带引号的字段按标准 CSV 规则处理
  （引号内的逗号不分割字段）。
- 列数不是 3、或引号不闭合等无法解析时返回 None。
- total 以**第 3 列**为金额：跳过空白字段，遇到非数字字段抛 ValueError。
"""

import csv


def parse(line):
    """解析一行 CSV，返回 3 个已裁剪的字符串组成的列表；不合法返回 None。"""
    try:
        rows = list(csv.reader([line]))
    except (csv.Error, StopIteration):
        return None
    if len(rows) != 1:
        return None
    fields = [f.strip() for f in rows[0]]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """累加行的第 3 列（金额）。空白字段跳过（贡献 0），非数字抛 ValueError。"""
    result = 0
    for r in rows:
        if len(r) < 3:
            raise ValueError("row has fewer than 3 columns: %r" % (r,))
        field = r[2].strip()
        if field == "":
            continue
        result += int(field)
    return result
