"""Stage 3: aggregate quantities by price."""


def total_by_price(rows):
    """Return ``{price: sum(qty)}`` over *rows*."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
