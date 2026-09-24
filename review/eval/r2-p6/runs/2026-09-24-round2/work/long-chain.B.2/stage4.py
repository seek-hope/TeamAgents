def report(totals):
    """Render totals as ascending-price lines '<price>:<qty>' with a trailing newline."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
