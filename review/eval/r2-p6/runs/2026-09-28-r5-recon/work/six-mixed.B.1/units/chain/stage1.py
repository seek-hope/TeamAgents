import csv


def parse_orders(path):
    with open(path, newline="") as f:
        reader = csv.DictReader(f)
        return [
            {"id": int(row["id"]), "qty": int(row["qty"]), "price": int(row["price"])}
            for row in reader
        ]
