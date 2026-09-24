from stage3 import total_by_price
def test_totals():
    rows = [{"id":1,"qty":2,"price":10},{"id":3,"qty":3,"price":7},{"id":4,"qty":1,"price":7}]
    assert total_by_price(rows) == {10:2, 7:4}
