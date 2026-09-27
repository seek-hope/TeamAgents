def total_by_price(rows):
    """按 price 汇总 qty，返回 {price: 总 qty}。"""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
