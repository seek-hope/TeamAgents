"""Stage 3: aggregate quantities per price."""


def total_by_price(rows):
    """Return ``{price: total_qty}`` summing the quantities of equal prices."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
