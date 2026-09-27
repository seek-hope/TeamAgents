def report(totals):
    """Render ``{price: total}`` as ``"price:total\\n"`` lines sorted by price."""
    return "".join(
        "{}:{}\n".format(price, totals[price])
        for price in sorted(totals)
    )
