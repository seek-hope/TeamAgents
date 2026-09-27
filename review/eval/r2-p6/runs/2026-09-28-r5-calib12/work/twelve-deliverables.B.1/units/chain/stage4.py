def report(totals):
    """把 {price: qty} 渲染成按 price 升序、每行 'price:qty\\n' 的字符串。"""
    return "".join("%d:%d\n" % (price, totals[price]) for price in sorted(totals))
