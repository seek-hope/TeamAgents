import csv


def parse_orders(path):
    """读取 orders.csv，把每行转成 {"id": int, "qty": int, "price": int}。"""
    rows = []
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            rows.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return rows
