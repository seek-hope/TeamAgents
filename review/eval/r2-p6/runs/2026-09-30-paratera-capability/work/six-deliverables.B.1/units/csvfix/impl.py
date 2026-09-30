"""CSV 解析与小计。

语义边界：
- `parse` 支持双引号字段（字段内逗号、`""` 转义），对未加引号的字段裁剪首尾空白；
  解析后列数必须恰好为 3，否则返回 ``None``。
- `total` 把每行第 3 列（索引 2）视为金额；空白金额跳过（不计、不报错）；
  非整数金额抛 ``ValueError``；列数不足 3 的行跳过。
"""


def parse(line):
    """把一行拆成恰好 3 个字段。

    - 未加引号的字段会裁剪首尾空白。
    - 双引号包裹的字段可以包含逗号，内部的 ``""`` 表示一个字面量引号。
    - 列数不是 3 时返回 ``None``。
    """
    if line is None:
        return None

    fields = []
    cur = []
    in_quotes = False
    i = 0
    n = len(line)
    while i < n:
        ch = line[i]
        if in_quotes:
            if ch == '"':
                if i + 1 < n and line[i + 1] == '"':
                    cur.append('"')
                    i += 2
                    continue
                in_quotes = False
                i += 1
                continue
            cur.append(ch)
            i += 1
        else:
            if ch == '"':
                in_quotes = True
                i += 1
            elif ch == ",":
                fields.append("".join(cur))
                cur = []
                i += 1
            else:
                cur.append(ch)
                i += 1
    fields.append("".join(cur))

    fields = [f.strip() for f in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """求各行金额（第 3 列）之和。

    - 空白金额字段被跳过。
    - 非整数金额抛 ``ValueError``。
    - 列数不足 3 的行被跳过。
    """
    result = 0
    for r in rows:
        if len(r) < 3:
            continue
        raw = r[2].strip()
        if raw == "":
            continue
        try:
            result += int(raw)
        except ValueError:
            raise ValueError("bad amount: %r" % (raw,))
    return result
