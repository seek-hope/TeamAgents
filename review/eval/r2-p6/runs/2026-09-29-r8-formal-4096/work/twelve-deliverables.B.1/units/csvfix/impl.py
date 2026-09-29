import csv


def parse(line):
    """把一行 CSV 解析成字段列表：裁剪每个字段两端空白、处理引号，
    并且只在恰好 3 列时返回列表，否则返回 None。"""
    row = next(csv.reader([line]))
    fields = [f.strip() for f in row]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """对行中第三列（金额）求和：空字段跳过，非数字字段按 int() 报错。"""
    result = 0
    for r in rows:
        if r is None or len(r) < 3:
            continue
        if r[2] == "":
            continue
        result += int(r[2])
    return result
