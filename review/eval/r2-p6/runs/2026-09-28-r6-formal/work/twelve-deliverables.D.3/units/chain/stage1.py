import csv


def parse_orders(path, **kwargs):
    """Read a CSV of orders into a list of int-valued dicts."""
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        return [
            {"id": int(row["id"]), "qty": int(row["qty"]), "price": int(row["price"])}
            for row in reader
        ]
