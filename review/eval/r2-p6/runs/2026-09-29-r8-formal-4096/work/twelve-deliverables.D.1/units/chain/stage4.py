"""Stage 4: render the totals as a report string."""


def report(totals):
    """Render ``{price: total_qty}`` as ``"{price}:{total}\\n"`` lines, price ascending."""
    return "".join(
        "{0}:{1}\n".format(price, totals[price])
        for price in sorted(totals)
    )
