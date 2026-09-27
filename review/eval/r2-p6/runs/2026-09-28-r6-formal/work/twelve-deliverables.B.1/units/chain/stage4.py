def report(totals):
    """把 {price: qty} 渲染成按 price 升序的 "price:qty\\n" 文本。"""
    return "".join("%s:%s\n" % (price, totals[price]) for price in sorted(totals))
