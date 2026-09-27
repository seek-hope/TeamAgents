def total_by_price(rows):
    """按 price 分组，累加各组的 qty，返回 {price: total_qty}。"""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
