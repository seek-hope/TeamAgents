"""Stage 4: render the totals mapping as a text report."""


def report(totals):
    """Return ``"<price>:<qty>"`` lines sorted by ascending price, newline-terminated."""
    lines = [f"{price}:{totals[price]}" for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
