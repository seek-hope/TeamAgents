"""CSV 行解析与金额汇总。

一行固定为 3 列：``名称,数量,单价``。第三列不再被直接当成金额；
每行的金额是 ``数量 * 单价``。
"""


def parse(line):
    """把一行 CSV 解析成 3 个字段的列表，不合法时返回 ``None``。

    语义边界：
      - 支持双引号包裹的字段：引号内的逗号不拆分，``""`` 表示一个字面引号；
      - 引号本身不是数据，解析后被去掉；
      - 每个字段去除首尾空白；
      - 列数不是 3 时返回 ``None``（列数不合法）；
      - 引号未闭合时返回 ``None``；
      - 非字符串输入返回 ``None``。
    """
    if not isinstance(line, str):
        return None

    fields = []
    buf = []
    in_quotes = False
    i = 0
    while i < len(line):
        ch = line[i]
        if in_quotes:
            if ch == '"':
                if i + 1 < len(line) and line[i + 1] == '"':
                    buf.append('"')
                    i += 2
                    continue
                in_quotes = False
            else:
                buf.append(ch)
        else:
            if ch == '"' and not "".join(buf).strip():
                # 字段以引号开头（允许前面有空白）：引号内的逗号不拆分。
                buf = []
                in_quotes = True
            elif ch == ",":
                fields.append("".join(buf))
                buf = []
            else:
                buf.append(ch)
        i += 1

    if in_quotes:  # 引号未闭合
        return None

    fields.append("".join(buf))
    fields = [f.strip() for f in fields]
    if len(fields) != 3:
        return None
    return fields


def _amount_field(value):
    """把数量/单价字段转成 int；空白返回 ``None``。"""
    if isinstance(value, str):
        value = value.strip()
    if value == "":
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        raise ValueError("invalid amount field: %r" % (value,))


def total(rows):
    """把每行的 ``数量 * 单价`` 相加。

    语义边界：
      - 列的约定是 ``[名称, 数量, 单价]``，金额 = 数量 * 单价（不是第三列本身）；
      - 数量或单价为空白的行被跳过（不当作 0，也不参与合计）；
      - 非空但无法转成整数的字段抛 ``ValueError``（报错而不是静默当 0）；
      - 列数不是 3 的行抛 ``ValueError``（结构不合法）。
    """
    result = 0
    for row in rows:
        if len(row) != 3:
            raise ValueError("row must have exactly 3 columns: %r" % (row,))
        qty = _amount_field(row[1])
        price = _amount_field(row[2])
        if qty is None or price is None:
            continue
        result += qty * price
    return result
