"""Stage 1: parse the orders CSV into typed row dictionaries."""

import csv


def parse_orders(path):
    """Read a CSV with header ``id,qty,price`` and return a list of rows.

    Each row is ``{"id": int, "qty": int, "price": int}`` in file order.
    """
    rows = []
    with open(path, newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        for record in reader:
            rows.append(
                {
                    "id": int(record["id"]),
                    "qty": int(record["qty"]),
                    "price": int(record["price"]),
                }
            )
    return rows
