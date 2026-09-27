from stage1 import parse_orders
def test_parse():
    rows = parse_orders("data/orders.csv")
    assert rows == [{"id":1,"qty":2,"price":10},{"id":2,"qty":0,"price":5},{"id":3,"qty":3,"price":7},{"id":4,"qty":1,"price":7}], rows
