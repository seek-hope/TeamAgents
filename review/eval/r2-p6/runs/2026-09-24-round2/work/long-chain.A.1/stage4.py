"""Stage 4: render the totals map as a newline-terminated report."""


def report(totals):
    """Return ``"<price>:<qty>\\n"`` lines sorted by price ascending."""
    lines = ["{}:{}".format(price, totals[price]) for price in sorted(totals)]
    if not lines:
        return ""
    return "\n".join(lines) + "\n"
