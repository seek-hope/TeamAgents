"""Stage 1: read orders from a CSV file.

The CSV is expected to have a header row (e.g. ``id,qty,price``) followed by
rows of integer values.  Rows are returned as dicts in file order.
"""

import csv


def parse_orders(path):
    """Parse the CSV at ``path`` into a list of order dicts.

    The header row is skipped; every remaining non-empty row becomes
    ``{"id": int, "qty": int, "price": int}``, preserved in file order.
    """
    orders = []
    with open(path, newline="") as handle:
        reader = csv.reader(handle)
        next(reader, None)  # skip the header row
        for row in reader:
            if not row or all(not cell.strip() for cell in row):
                continue
            order_id, qty, price = (cell.strip() for cell in row[:3])
            orders.append({"id": int(order_id), "qty": int(qty), "price": int(price)})
    return orders
