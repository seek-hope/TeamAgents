def filter_orders(rows):
    """只保留数量大于 0 的行，保持原有顺序。"""
    return [row for row in rows if row["qty"] > 0]
