"""Stage 1: read the orders CSV into a list of int-valued dicts."""

import csv


def parse_orders(path):
    """Read the CSV at *path* (header ``id,qty,price``).

    Returns a list of ``{"id": int, "qty": int, "price": int}`` dicts, one per
    data row, in file order.
    """
    rows = []
    with open(path, newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        for record in reader:
            rows.append(
                {
                    "id": int(record["id"]),
                    "qty": int(record["qty"]),
                    "price": int(record["price"]),
                }
            )
    return rows
