def filter_orders(rows):
    """Drop rows whose ``qty`` is <= 0, preserving the original order."""
    return [row for row in rows if row["qty"] > 0]
