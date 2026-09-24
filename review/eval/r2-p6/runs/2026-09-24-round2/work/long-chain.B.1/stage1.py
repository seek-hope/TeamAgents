"""Stage 1: parse the orders CSV into typed dictionaries."""

import csv


def parse_orders(path):
    """Read a CSV with header ``id,qty,price`` and return a list of dicts.

    Every value is converted to ``int``. Row order is preserved exactly as it
    appears in the file.
    """
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
