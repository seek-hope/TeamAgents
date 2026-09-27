import csv
import io


def parse(line):
    """解析一行 CSV（恰好 3 列）。

    - 支持双引号包裹的字段（字段内的逗号不算分隔符）。
    - 每个字段去掉首尾空白。
    - 列数不是 3 时返回 None。
    """
    row = next(csv.reader(io.StringIO(line)), [])
    row = [field.strip() for field in row]
    if len(row) != 3:
        return None
    return row


def total(rows):
    """把每行的 (name, qty, price) 折算成金额 qty * price 后求和。

    - 空字段（qty 或 price 为空）的行跳过，不参与统计。
    - 非空但不是整数的字段抛出 ValueError。
    """
    result = 0
    for row in rows:
        if len(row) != 3:
            raise ValueError(f"expected 3 columns, got {len(row)}: {row!r}")
        _, qty, price = row
        if qty == "" or price == "":
            continue
        try:
            result += int(qty) * int(price)
        except ValueError as exc:
            raise ValueError(f"bad numeric field in {row!r}") from exc
    return result
