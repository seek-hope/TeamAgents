from stage2 import filter_orders
def test_filter():
    rows = [{"id":1,"qty":2,"price":10},{"id":2,"qty":0,"price":5}]
    assert filter_orders(rows) == [{"id":1,"qty":2,"price":10}]
