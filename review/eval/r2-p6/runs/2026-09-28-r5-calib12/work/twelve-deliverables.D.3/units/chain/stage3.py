"""Stage 3: aggregate the surviving orders by unit price.

The result maps each distinct ``price`` to the total quantity ordered at that
price, e.g. orders ``(price=10, qty=2)`` and ``(price=7, qty=3)`` plus
``(price=7, qty=1)`` produce ``{10: 2, 7: 4}``.
"""


def total_by_price(rows):
    """Return ``{price: total_qty}`` for the orders in ``rows``."""
    totals = {}
    for row in rows:
        price = row["price"]
        totals[price] = totals.get(price, 0) + row["qty"]
    return totals
