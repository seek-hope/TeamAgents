"""Stage 4: render the aggregated totals as a report string."""


def report(totals):
    """Format *totals* as one ``"price:total"`` line per price.

    Lines are sorted by price ascending and each line is terminated by ``"\\n"``.
    """
    lines = ["%d:%d" % (price, totals[price]) for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
