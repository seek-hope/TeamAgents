"""Stage 2: keep only the orders that actually have a quantity.

An order with ``qty == 0`` contributes nothing downstream, so it is dropped.
Input and output are both lists of ``{"id", "qty", "price"}`` dicts and the
relative order of the surviving orders is preserved.
"""


def filter_orders(rows):
    """Return the orders from ``rows`` whose ``qty`` is greater than zero."""
    return [row for row in rows if row["qty"] > 0]
