"""Stage 4: render the totals as a newline-terminated report."""


def report(totals):
    """Render ``"<price>:<qty>"`` per line, prices ascending, trailing newline."""
    lines = [f"{price}:{totals[price]}" for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
