def report(totals):
    """把 {price: qty} 渲染成按 price 升序的 "price:qty\\n" 拼接串。"""
    return "".join("%d:%d\n" % (price, totals[price]) for price in sorted(totals))
