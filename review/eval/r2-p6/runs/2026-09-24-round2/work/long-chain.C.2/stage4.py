"""Stage 4: render the aggregated totals as a report string."""


def report(totals):
    """Return ``"<price>:<qty>"`` lines sorted by price ascending.

    Every line (including the last one) ends with a newline.
    """
    lines = ["%d:%d" % (price, totals[price]) for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
