"""Stage 4: render the price totals as the final report text.

Each ``price: total`` pair goes on its own line, sorted by price ascending, and
the report always ends with a trailing newline.  For ``{10: 2, 7: 4}`` the
report is ``"7:4\\n10:2\\n"``.
"""


def report(totals):
    """Return the sorted, newline-terminated text for a ``{price: qty}`` map."""
    return "".join(
        "{0}:{1}\n".format(price, totals[price]) for price in sorted(totals)
    )
