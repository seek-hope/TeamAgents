def report(totals):
    """Render {price: qty} as ascending-price lines "<price>:<qty>", newline-terminated."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
