def report(totals):
    """把 ``{price: qty}`` 渲染成按价格升序的文本，每行 ``price:qty``。

    语义边界：行尾带换行；价格按数值升序；空字典返回空字符串。
    """
    return "".join("%d:%d\n" % (price, totals[price]) for price in sorted(totals))
