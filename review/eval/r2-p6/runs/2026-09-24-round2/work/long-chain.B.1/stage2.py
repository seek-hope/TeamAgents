"""Stage 2: drop rows that carry no quantity."""


def filter_orders(rows):
    """Return the rows with ``qty > 0``, preserving the original order."""
    return [row for row in rows if row["qty"] > 0]
