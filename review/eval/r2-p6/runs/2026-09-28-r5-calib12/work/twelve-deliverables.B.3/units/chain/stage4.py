def report(totals):
    """把 {price: qty} 渲染成按 price 升序、每行 "price:qty\\n" 的文本。"""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
