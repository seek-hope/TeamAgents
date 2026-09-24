"""Stage 3: aggregate quantity per price."""


def total_by_price(rows):
    """Return ``{price: total qty}``, summing qty for equal prices."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
