def total_by_price(rows):
    """Sum ``qty`` per ``price`` and return a ``{price: total_qty}`` dict."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
