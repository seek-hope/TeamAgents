"""Stage 3: aggregate quantities per price."""


def total_by_price(rows):
    """Return ``{price: total_qty}``, summing ``qty`` of rows sharing a price."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
