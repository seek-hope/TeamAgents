"""Stage 2: drop non-positive quantities, preserving input order."""


def filter_orders(rows):
    """Return the rows whose ``qty`` is greater than zero, in original order."""
    return [row for row in rows if row["qty"] > 0]
