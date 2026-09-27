import csv


def parse_orders(path):
    """Read a CSV file with ``id,qty,price`` columns into a list of dicts.

    Numeric fields are converted to ints. The header row is skipped.
    """
    rows = []
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            rows.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return rows
