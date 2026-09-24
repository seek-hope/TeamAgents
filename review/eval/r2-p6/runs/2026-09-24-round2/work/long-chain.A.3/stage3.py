"""Stage 3: aggregate quantities per unit price.

Semantics
---------
``total_by_price(rows)`` sums the ``qty`` of all rows sharing the same
``price`` and returns a ``{price: total_qty}`` dict.  An empty input yields an
empty dict.
"""


def total_by_price(rows):
    """Aggregate ``qty`` keyed by ``price``."""
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
