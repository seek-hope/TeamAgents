def filter_orders(rows):
    return [row for row in rows if row["qty"] > 0]
