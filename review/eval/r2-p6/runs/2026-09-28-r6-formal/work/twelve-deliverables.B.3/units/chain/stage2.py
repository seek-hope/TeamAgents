def filter_orders(rows):
    """只保留 qty 大于 0 的订单行（保持原顺序）。"""
    return [row for row in rows if row["qty"] > 0]
