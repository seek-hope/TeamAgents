def report(totals):
    """Render `price:total` lines sorted by price ascending."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
