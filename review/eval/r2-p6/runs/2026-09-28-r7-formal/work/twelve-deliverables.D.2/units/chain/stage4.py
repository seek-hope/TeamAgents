"""Stage 4: render the aggregated totals as a report string."""


def report(totals):
    """Return one ``"price:qty\\n"`` line per price, sorted by price ascending."""
    return "".join(
        "{0}:{1}\n".format(price, totals[price]) for price in sorted(totals)
    )
