import csv


def parse(line):
    """解析一行 CSV：按逗号切分（支持双引号包裹的字段），裁剪每个字段两端空白，
    并要求恰好 3 列；列数不是 3 时返回 None。"""
    fields = next(csv.reader([line]))
    fields = [f.strip() for f in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """把每行第三列当作金额（单位：分）求和，跳过空字段；
    非空但无法解析为整数的字段报 ValueError。"""
    result = 0
    for row in rows:
        if len(row) <= 2:
            continue
        value = row[2].strip()
        if value == "":
            continue
        try:
            result += int(value)
        except ValueError:
            raise ValueError("bad amount: %r" % (row[2],))
    return result
