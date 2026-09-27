import csv
import io


def parse(line):
    """把一行 CSV 解析成恰好 3 列的列表：去空白、支持引号。

    列数不是 3 时返回 None；引号不闭合等解析错误也返回 None。
    """
    try:
        fields = next(csv.reader(io.StringIO(line)))
    except (csv.Error, StopIteration):
        return None
    fields = [field.strip() for field in fields]
    if len(fields) != 3:
        return None
    return fields


def total(rows):
    """把每行第三列（金额）相加，空字段跳过。

    列数不是 3、或金额不是整数时抛 ValueError。
    """
    total_sum = 0
    for row in rows:
        if len(row) != 3:
            raise ValueError(f"expected 3 columns, got {len(row)}: {row!r}")
        cell = row[2].strip()
        if not cell:
            continue
        total_sum += int(cell)
    return total_sum
