def filter_orders(rows):
    """Keep only rows with qty > 0."""
    return [row for row in rows if row["qty"] > 0]
