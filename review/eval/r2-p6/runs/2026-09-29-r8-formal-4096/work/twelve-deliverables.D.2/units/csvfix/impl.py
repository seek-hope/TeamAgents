import csv
import io

#: 每条记录必须有 3 列（名称 / 数量 / 金额），否则视为非法记录。
COLUMN_COUNT = 3

#: 金额所在列的索引（0-based）。
AMOUNT_COLUMN = 2


def parse(line):
    """把一行 CSV 解析成字段列表。

    语义：
    * 支持引号包裹的字段（字段内可包含逗号），并按 csv 规则去除引号；
    * 每个字段两端空白被去除（trim）；
    * 若字段数不等于 3（``COLUMN_COUNT``），返回 ``None``。
    """
    if line is None:
        return None

    reader = csv.reader(io.StringIO(line))
    try:
        fields = next(reader)
    except StopIteration:
        fields = []

    fields = [field.strip() for field in fields]
    if len(fields) != COLUMN_COUNT:
        return None
    return fields


def total(rows):
    """求所有记录中金额列（``AMOUNT_COLUMN``）之和。

    语义：
    * 只统计第 3 列（索引 2）的金额，不把其他列当作金额；
    * 空字段（空串或纯空白）被跳过，不参与求和；
    * 列数不足的记录被跳过。
    """
    total_amount = 0
    for row in rows:
        if row is None or len(row) < COLUMN_COUNT:
            continue
        amount = row[AMOUNT_COLUMN]
        if amount is None:
            continue
        amount = str(amount).strip()
        if not amount:
            continue
        total_amount += int(amount)
    return total_amount
