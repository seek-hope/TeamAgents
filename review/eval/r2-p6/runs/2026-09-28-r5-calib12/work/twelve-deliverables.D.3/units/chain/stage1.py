"""Stage 1: parse the raw orders CSV into a list of order dicts.

The CSV has a header row (``id,qty,price``) followed by one order per line.
Every field is an integer, and the returned list preserves the file order.
Orders with ``qty == 0`` are kept here; dropping them is stage 2's job.
"""

import csv


def parse_orders(path):
    """Read ``path`` and return ``[{"id": int, "qty": int, "price": int}, ...]``.

    The header line is consumed but not emitted, and blank lines are ignored.
    """
    orders = []
    with open(path, newline="") as handle:
        reader = csv.DictReader(handle)
        for row in reader:
            if row is None or row.get("id") in (None, ""):
                continue
            orders.append(
                {
                    "id": int(row["id"]),
                    "qty": int(row["qty"]),
                    "price": int(row["price"]),
                }
            )
    return orders
