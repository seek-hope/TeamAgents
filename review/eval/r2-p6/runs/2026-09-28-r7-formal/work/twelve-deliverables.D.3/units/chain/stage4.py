def report(totals):
    """Render {price: qty} as one "price:qty\\n" line per price, ascending by price."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
