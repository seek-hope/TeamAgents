def parse(line):
    """把一行 CSV 拆成 3 个字段：逐个裁剪空白，列数不是 3 则返回 None。"""
    fields = [f.strip() for f in line.split(",")]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """把第三列当金额求和，跳过空金额字段（含纯空白）。"""
    result = 0
    for row in rows:
        if len(row) > 2 and row[2].strip():
            result += int(row[2])
    return result
