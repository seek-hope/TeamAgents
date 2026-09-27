def report(totals):
    """Render "price:total\\n" lines ordered by price ascending."""
    return "".join(
        "%d:%d\n" % (price, totals[price]) for price in sorted(totals)
    )
