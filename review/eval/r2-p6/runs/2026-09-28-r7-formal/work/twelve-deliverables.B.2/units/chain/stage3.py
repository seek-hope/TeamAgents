def total_by_price(rows):
    """按 price 汇总 qty，返回 {price: 总数量}。"""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
