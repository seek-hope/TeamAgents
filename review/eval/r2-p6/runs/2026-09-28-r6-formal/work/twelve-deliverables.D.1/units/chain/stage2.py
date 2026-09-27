def filter_orders(rows):
    """Keep only orders with a positive quantity, preserving order."""
    return [row for row in rows if row["qty"] > 0]
