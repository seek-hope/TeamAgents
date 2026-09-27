def filter_orders(rows):
    """只保留 qty 大于 0 的订单，保持原有顺序。"""
    return [row for row in rows if row["qty"] > 0]
