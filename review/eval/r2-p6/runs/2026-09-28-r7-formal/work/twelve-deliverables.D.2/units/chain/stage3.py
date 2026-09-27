"""Stage 3: aggregate quantities per unit price."""


def total_by_price(rows):
    """Map each ``price`` to the sum of ``qty`` of all rows with that price."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
