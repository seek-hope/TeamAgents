def total_by_price(rows):
    """Sum ``qty`` grouped by ``price``."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
