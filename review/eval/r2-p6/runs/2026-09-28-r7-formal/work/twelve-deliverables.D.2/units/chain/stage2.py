"""Stage 2: drop orders that have no quantity."""


def filter_orders(rows):
    """Return only the rows whose ``qty`` is greater than zero.

    Order and row content are preserved unchanged.
    """
    return [row for row in rows if row["qty"] > 0]
