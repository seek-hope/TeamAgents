def parse(line):
    """BUG: 不处理引号、不裁剪空白、不校验列数。"""
    return line.split(",")

def total(rows):
    """BUG: 空字段当作 0 但不报错，且把第三列当成金额。"""
    return sum(int(r[2]) for r in rows if len(r) > 2 and r[2])
