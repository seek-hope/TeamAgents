"""Stage 3: aggregate total quantity per price."""


def total_by_price(rows):
    """Return ``{price: total_qty}``, summing ``qty`` across equal prices."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
