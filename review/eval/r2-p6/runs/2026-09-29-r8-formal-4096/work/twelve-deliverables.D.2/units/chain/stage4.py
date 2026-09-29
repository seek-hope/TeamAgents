def report(totals):
    """Format ``{price: total}`` as ``"price:total\\n"`` lines, price ascending."""
    return "".join("%s:%s\n" % (price, totals[price]) for price in sorted(totals))
