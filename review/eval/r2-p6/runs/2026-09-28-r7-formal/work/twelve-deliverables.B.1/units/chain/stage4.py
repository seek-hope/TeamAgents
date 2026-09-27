def report(totals):
    """Render ``price:qty`` lines ordered by ascending price."""
    return "".join("%s:%s\n" % (price, totals[price]) for price in sorted(totals))
