def filter_orders(rows):
    """Return only the rows whose ``qty`` is not zero."""
    return [row for row in rows if row["qty"] != 0]
