def total_by_price(rows):
    """按 price 分组，累加 qty，返回 {price: total_qty}。"""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
