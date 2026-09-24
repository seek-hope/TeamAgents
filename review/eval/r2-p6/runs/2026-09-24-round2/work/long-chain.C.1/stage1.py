"""Stage 1: parse the orders CSV into a list of dicts."""

import csv


def parse_orders(path):
    """Read ``path`` (header ``id,qty,price``) and return row dicts.

    Each row is ``{"id": int, "qty": int, "price": int}`` in file order.
    """
    rows = []
    with open(path, newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        for raw in reader:
            rows.append(
                {
                    "id": int(raw["id"]),
                    "qty": int(raw["qty"]),
                    "price": int(raw["price"]),
                }
            )
    return rows
