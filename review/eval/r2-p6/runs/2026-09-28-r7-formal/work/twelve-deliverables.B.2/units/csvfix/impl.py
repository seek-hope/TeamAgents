import csv


def parse(line):
    """把一行 CSV 解析成三个字段。

    - 逐字段裁剪首尾空白。
    - 支持双引号包裹的字段（字段内的逗号不会拆分）。
    - 列数不是 3 时返回 None。
    """
    try:
        fields = next(csv.reader([line], skipinitialspace=True))
    except (csv.Error, StopIteration):
        return None
    fields = [f.strip() for f in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """把每行第三列当作金额求和，跳过空字段与列数不足的行。"""
    total_cents = 0
    for r in rows:
        if len(r) <= 2:
            continue
        field = r[2].strip()
        if field == "":
            continue
        total_cents += int(field)
    return total_cents
