def total_by_price(rows):
    totals = {}
    for r in rows:
        totals[r["price"]] = totals.get(r["price"], 0) + r["qty"]
    return totals
