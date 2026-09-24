"""Stage 2: drop non-positive quantities.

Semantics
---------
``filter_orders(rows)`` returns a new list containing only the rows whose
``qty`` is strictly positive (``qty > 0``).  Input order is preserved and the
original list is left untouched.
"""


def filter_orders(rows):
    """Return the rows with ``qty > 0``, keeping the original order."""
    return [row for row in rows if row["qty"] > 0]
