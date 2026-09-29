"""Stage 3: aggregate total quantity per price."""


def total_by_price(rows):
    """Group ``rows`` by price and sum the quantities -> ``{price: total_qty}``."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
