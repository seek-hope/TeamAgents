import csv


def parse_orders(path):
    """Read a CSV with header ``id,qty,price`` and return a list of dicts.

    Each row is returned as ``{"id": int, "qty": int, "price": int}`` in the
    order it appears in the file.
    """
    rows = []
    with open(path, newline="") as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(
                {
                    "id": int(row["id"]),
                    "qty": int(row["qty"]),
                    "price": int(row["price"]),
                }
            )
    return rows
