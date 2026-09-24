"""Stage 4: render the aggregated totals as a report string.

Semantics
---------
``report(totals)`` takes a ``{price: qty}`` mapping and returns a string with
one line per price, formatted as ``"<price>:<qty>"``, sorted by price in
ascending order.  The string always ends with a trailing newline.  For an
empty mapping the result is the empty string.
"""


def report(totals):
    """Render ``totals`` as price-ascending ``"<price>:<qty>"`` lines."""
    return "".join(f"{price}:{totals[price]}\n" for price in sorted(totals))
