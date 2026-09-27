import csv


def parse(line):
    """把一行 CSV 解析成恰好 3 个字段的列表。

    - 支持双引号包裹的字段（字段内部可以包含逗号）。
    - 每个字段两侧的空白会被裁剪。
    - 字段数不是 3 时返回 None（不抛异常）。
    """
    if not isinstance(line, str):
        return None
    try:
        row = next(csv.reader([line]))
    except (csv.Error, StopIteration):
        return None
    fields = [field.strip() for field in row]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """求和每行的金额（金额 = 数量 * 单价）。

    rows 中每行形如 [id, qty, price]（parse 的输出，均为字符串）。
    - 单价为空白（""）的行视为无金额，跳过。
    - 单价非空但不能解析成整数的行抛 ValueError（不再静默当作 0）。
    - 行长度不足 3 时跳过。
    """
    result = 0
    for row in rows:
        if len(row) < 3:
            continue
        price_text = str(row[2]).strip()
        if price_text == "":
            continue
        try:
            qty = int(str(row[1]).strip())
            price = int(price_text)
        except ValueError as exc:
            raise ValueError("non-integer quantity/price in row %r" % (row,)) from exc
        result += qty * price
    return result
