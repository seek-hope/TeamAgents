"""Stage 4: render the aggregated totals as a report string."""


def report(totals):
    """Return ``"<price>:<qty>"`` lines sorted by ascending price."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
