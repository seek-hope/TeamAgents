import csv


def parse_orders(path):
    """Read the orders CSV (header: id,qty,price) into a list of dicts with int values."""
    rows = []
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            rows.append({key: int(value) for key, value in row.items()})
    return rows
