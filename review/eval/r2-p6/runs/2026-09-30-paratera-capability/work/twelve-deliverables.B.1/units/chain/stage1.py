import csv


def parse_orders(path):
    """读取 CSV（表头 id,qty,price），返回 [{"id":int,"qty":int,"price":int}, ...]。"""
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
