def total_by_price(rows):
    """Sum qty grouped by price."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
