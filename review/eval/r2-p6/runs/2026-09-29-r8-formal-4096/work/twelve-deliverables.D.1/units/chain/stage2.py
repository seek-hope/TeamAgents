"""Stage 2: keep only orders with a positive quantity."""


def filter_orders(rows):
    """Return the rows whose ``qty`` is greater than zero, order preserved."""
    return [row for row in rows if row["qty"] > 0]
