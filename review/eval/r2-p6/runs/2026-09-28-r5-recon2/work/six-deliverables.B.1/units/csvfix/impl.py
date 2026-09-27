"""CSV 小工具。

每一行有且仅有 3 列；字段两侧的空白在未加引号时会被裁剪。
双引号包裹的字段里可以包含逗号；两个连续的双引号表示一个字面双引号。
"""

DATA_COLUMNS = 3
AMOUNT_INDEX = 2


def _split_fields(line):
    """按 RFC4180 风格切分一行，返回 [(值, 是否被引号包裹), ...]。"""
    fields = []
    buf = []
    quoted = False
    in_quotes = False
    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if in_quotes:
            if ch == '"':
                if i + 1 < n and line[i + 1] == '"':
                    buf.append('"')
                    i += 2
                else:
                    in_quotes = False
                    i += 1
            else:
                buf.append(ch)
                i += 1
        else:
            if ch == ",":
                fields.append(("".join(buf), quoted))
                buf = []
                quoted = False
                i += 1
            elif ch == '"' and not buf and not quoted:
                in_quotes = True
                quoted = True
                i += 1
            else:
                buf.append(ch)
                i += 1
    fields.append(("".join(buf), quoted))
    return fields


def parse(line):
    """把一行切成 3 个字段。

    - 未加引号的字段会裁剪两侧空白；引号字段保留原样。
    - 列数不是 3 时返回 None（不抛异常）。
    """
    values = []
    for value, was_quoted in _split_fields(line):
        values.append(value if was_quoted else value.strip())
    if len(values) != DATA_COLUMNS:
        return None
    return values


def total(rows):
    """累加每行金额列（第三列）。

    - 金额为空字符串的行被跳过。
    - 金额是非整数时抛 ValueError。
    """
    result = 0
    for row in rows:
        if len(row) <= AMOUNT_INDEX:
            continue
        raw = row[AMOUNT_INDEX]
        if raw == "" or raw is None:
            continue
        try:
            result += int(raw)
        except (TypeError, ValueError):
            raise ValueError("bad amount: %r" % (raw,))
    return result
