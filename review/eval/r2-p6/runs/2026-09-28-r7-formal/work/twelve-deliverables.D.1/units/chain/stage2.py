def filter_orders(rows):
    return [r for r in rows if r["qty"] > 0]
