import csv


def parse_orders(path):
    """Read orders CSV into a list of dicts with int id/qty/price."""
    rows = []
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            rows.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return rows
