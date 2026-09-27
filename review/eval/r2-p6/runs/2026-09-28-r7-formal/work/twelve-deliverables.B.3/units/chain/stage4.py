def report(totals):
    """把 {price: total_qty} 渲染成按 price 升序的 "price:qty\\n" 文本。"""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
