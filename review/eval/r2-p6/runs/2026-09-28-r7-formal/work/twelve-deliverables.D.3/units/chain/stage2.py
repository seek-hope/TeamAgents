def filter_orders(rows):
    """Keep only the rows whose qty is greater than zero."""
    return [row for row in rows if row["qty"] > 0]
