import csv


def parse_orders(path):
    rows = []
    with open(path, newline="") as f:
        for rec in csv.DictReader(f):
            rows.append(
                {
                    "id": int(rec["id"]),
                    "qty": int(rec["qty"]),
                    "price": int(rec["price"]),
                }
            )
    return rows
