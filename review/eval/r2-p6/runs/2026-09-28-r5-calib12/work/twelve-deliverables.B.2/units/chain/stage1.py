import csv


def parse_orders(path):
    """读取订单 CSV，返回 ``[{"id": int, "qty": int, "price": int}, ...]``。

    语义边界：
      - 首行是表头 ``id,qty,price``（用 ``csv.DictReader`` 读取）；
      - 三列都转换成 ``int``；
      - 保持文件中的行顺序。
    """
    orders = []
    with open(path, newline="") as fh:
        for row in csv.DictReader(fh):
            orders.append({
                "id": int(row["id"]),
                "qty": int(row["qty"]),
                "price": int(row["price"]),
            })
    return orders
