import csv


def parse(line):
    """按 CSV 语义解析一行：去空白、支持引号，列数必须是 3，否则返回 None。"""
    try:
        fields = next(csv.reader([line]))
    except (csv.Error, StopIteration):
        return None
    fields = [field.strip() for field in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """累加每行第三列的金额（整数）。空字段跳过，非法数字抛 ValueError。"""
    total_amount = 0
    for row in rows:
        if len(row) < 3:
            continue
        raw = row[2].strip()
        if not raw:
            continue
        total_amount += int(raw)
    return total_amount
