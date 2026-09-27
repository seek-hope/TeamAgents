def total_by_price(rows):
    """Map each price to the total qty of the rows sharing that price."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
