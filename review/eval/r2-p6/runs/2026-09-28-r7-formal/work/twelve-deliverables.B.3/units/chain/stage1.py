import csv


def parse_orders(path):
    """读取订单 CSV（表头 id,qty,price），返回 [{"id": int, "qty": int, "price": int}, ...]。"""
    orders = []
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            orders.append(
                {"id": int(row["id"]), "qty": int(row["qty"]), "price": int(row["price"])}
            )
    return orders
