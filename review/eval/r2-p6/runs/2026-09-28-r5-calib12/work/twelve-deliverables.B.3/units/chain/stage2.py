def filter_orders(rows):
    """保留数量大于 0 的订单。"""
    return [row for row in rows if row["qty"] > 0]
