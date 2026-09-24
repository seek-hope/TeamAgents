def report(totals):
    """Render ``{price: qty}`` as ``"<price>:<qty>"`` lines sorted by price.

    The returned string ends with a trailing newline.
    """
    lines = [f"{price}:{totals[price]}" for price in sorted(totals)]
    return "".join(line + "\n" for line in lines)
