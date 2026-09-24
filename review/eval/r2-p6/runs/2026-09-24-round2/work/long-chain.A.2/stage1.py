"""Stage 1: read the orders CSV into a list of dicts."""

import csv


def parse_orders(path):
    """Read ``path`` (header ``id,qty,price``) and return rows as
    ``[{"id": int, "qty": int, "price": int}, ...]`` in file order."""
    rows = []
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            rows.append(
                {
                    "id": int(row["id"]),
                    "qty": int(row["qty"]),
                    "price": int(row["price"]),
                }
            )
    return rows
