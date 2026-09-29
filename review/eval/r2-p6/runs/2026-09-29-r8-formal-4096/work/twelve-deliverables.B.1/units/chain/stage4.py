def report(totals):
    """把 {price: total} 渲染成按 price 升序的行，每行 "price:total\\n"。"""
    return "".join("%s:%s\n" % (price, totals[price]) for price in sorted(totals))
