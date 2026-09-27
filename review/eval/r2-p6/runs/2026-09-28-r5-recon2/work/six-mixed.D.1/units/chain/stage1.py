import csv


def parse_orders(path):
    """Read the orders CSV and return one dict per row, in file order.

    Each dict has the keys "id", "qty" and "price" with int values.
    """
    rows = []
    with open(path, newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        for raw in reader:
            rows.append(
                {
                    "id": int(raw["id"]),
                    "qty": int(raw["qty"]),
                    "price": int(raw["price"]),
                }
            )
    return rows
