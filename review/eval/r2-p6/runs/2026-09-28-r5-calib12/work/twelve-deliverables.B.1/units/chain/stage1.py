import csv


def parse_orders(path):
    """读取 CSV（表头 id,qty,price），返回整数字段的字典列表。"""
    rows = []
    with open(path, newline="") as handle:
        reader = csv.DictReader(handle)
        for record in reader:
            rows.append({
                "id": int(record["id"]),
                "qty": int(record["qty"]),
                "price": int(record["price"]),
            })
    return rows
