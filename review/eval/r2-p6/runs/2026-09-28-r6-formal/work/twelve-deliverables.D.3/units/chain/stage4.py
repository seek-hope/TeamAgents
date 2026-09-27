def report(totals):
    """Text lines `price:total\\n` sorted by price."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
