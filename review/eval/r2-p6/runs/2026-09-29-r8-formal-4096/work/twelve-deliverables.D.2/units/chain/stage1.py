import csv


def parse_orders(path):
    """Read a CSV of orders and return a list of dicts with int values.

    The file is expected to have a header row (e.g. ``id,qty,price``).
    Every field of every data row is converted to ``int``.
    """
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        rows = []
        for raw in reader:
            if raw is None:
                continue
            row = {key: int(value) for key, value in raw.items() if key is not None}
            if not row:
                continue
            rows.append(row)
    return rows
