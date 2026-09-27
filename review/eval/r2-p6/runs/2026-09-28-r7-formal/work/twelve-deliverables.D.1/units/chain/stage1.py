import csv


def parse_orders(csv_path):
    rows = []
    with open(csv_path, newline="") as f:
        for row in csv.DictReader(f):
            rows.append(
                {"id": int(row["id"]), "qty": int(row["qty"]), "price": int(row["price"])}
            )
    return rows
