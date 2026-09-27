def total_by_price(rows):
    """Sum quantities grouped by price."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
