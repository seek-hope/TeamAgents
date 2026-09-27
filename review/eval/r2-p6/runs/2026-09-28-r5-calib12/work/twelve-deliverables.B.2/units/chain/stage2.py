def filter_orders(rows):
    """只保留 ``qty > 0`` 的订单，保持原顺序。

    语义边界：数量为 0 或负数的行被丢弃；返回新列表，元素仍是原字典。
    """
    return [row for row in rows if row["qty"] > 0]
