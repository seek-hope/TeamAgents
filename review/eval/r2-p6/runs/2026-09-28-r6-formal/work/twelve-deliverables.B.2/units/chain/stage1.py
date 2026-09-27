import csv


def parse_orders(path):
    """读取 CSV（表头 id,qty,price），返回 [{"id": int, "qty": int, "price": int}, ...]。"""
    with open(path, newline="") as fh:
        return [
            {"id": int(row["id"]), "qty": int(row["qty"]), "price": int(row["price"])}
            for row in csv.DictReader(fh)
        ]
