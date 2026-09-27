import csv


def parse_orders(path):
    """读取表头为 id,qty,price 的 CSV，返回按文件顺序排列的 dict 列表（值转为 int）。"""
    rows = []
    with open(path, newline="") as handle:
        for row in csv.DictReader(handle):
            rows.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return rows
