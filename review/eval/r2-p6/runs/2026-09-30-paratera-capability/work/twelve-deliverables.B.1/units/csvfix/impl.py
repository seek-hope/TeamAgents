import csv


def parse(line):
    """把一行 CSV 解析成 3 列的列表；每列裁剪空白。
    列数不是 3 时返回 None。引号内的逗号/转义按标准 CSV 处理。"""
    fields = next(csv.reader([line]))
    if len(fields) != 3:
        return None
    return [f.strip() for f in fields]


def total(rows):
    """每行是 [名称, 数量, 单价]，返回所有行的 数量 * 单价 之和。
    数量或单价为空白的行被跳过；非整数会抛出 ValueError。"""
    total = 0
    for row in rows:
        if len(row) < 3:
            raise ValueError("row needs 3 columns: %r" % (row,))
        qty, price = row[1].strip(), row[2].strip()
        if qty == "" or price == "":
            continue
        total += int(qty) * int(price)
    return total
