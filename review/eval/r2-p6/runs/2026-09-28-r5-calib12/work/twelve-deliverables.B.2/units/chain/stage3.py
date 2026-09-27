def total_by_price(rows):
    """按单价汇总数量，返回 ``{price: 总数量}``。

    语义边界：同一 price 的 qty 相加；键是价格，值是数量之和。
    """
    totals = {}
    for row in rows:
        totals[row["price"]] = totals.get(row["price"], 0) + row["qty"]
    return totals
