def filter_orders(rows):
    """Drop orders with qty == 0."""
    return [row for row in rows if row["qty"] != 0]
