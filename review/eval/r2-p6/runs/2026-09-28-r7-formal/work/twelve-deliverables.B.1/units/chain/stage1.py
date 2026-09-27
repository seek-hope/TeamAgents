import csv


def parse_orders(path):
    """Read the orders CSV at ``path`` and return a list of dicts with int values."""
    orders = []
    with open(path, newline="") as handle:
        for row in csv.DictReader(handle):
            orders.append(
                {
                    "id": int(row["id"]),
                    "qty": int(row["qty"]),
                    "price": int(row["price"]),
                }
            )
    return orders
