import csv


def parse_orders(path):
    """读取 CSV 文件，返回 [{"id": int, "qty": int, "price": int}, ...]。"""
    rows = []
    with open(path, newline="", encoding="utf-8") as fh:
        reader = csv.DictReader(fh)
        for row in reader:
            rows.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return rows
