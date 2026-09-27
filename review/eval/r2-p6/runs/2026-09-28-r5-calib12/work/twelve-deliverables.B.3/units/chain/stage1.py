import csv


def parse_orders(path):
    """读取 CSV（表头 id,qty,price），返回 [{"id": int, "qty": int, "price": int}, ...]。"""
    rows = []
    with open(path, newline="") as f:
        reader = csv.DictReader(f)
        for raw in reader:
            rows.append({
                "id": int(raw["id"]),
                "qty": int(raw["qty"]),
                "price": int(raw["price"]),
            })
    return rows
