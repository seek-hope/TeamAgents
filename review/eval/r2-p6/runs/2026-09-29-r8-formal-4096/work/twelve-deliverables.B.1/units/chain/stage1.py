import csv


def parse_orders(path):
    """读取 CSV（表头 id,qty,price），把三列都转成 int，返回行字典列表。"""
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        return [
            {"id": int(row["id"]), "qty": int(row["qty"]), "price": int(row["price"])}
            for row in reader
        ]
