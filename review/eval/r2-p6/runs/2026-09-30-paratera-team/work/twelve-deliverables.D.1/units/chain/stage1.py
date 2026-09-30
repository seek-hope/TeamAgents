import csv


def parse_orders(path):
    rows = []
    with open(path, newline="") as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return rows
