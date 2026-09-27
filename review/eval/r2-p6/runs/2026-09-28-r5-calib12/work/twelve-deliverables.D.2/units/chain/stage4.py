def report(totals):
    """Render "price:qty\\n" lines sorted ascending by price."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
