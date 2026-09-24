from ledger import Ledger, balance
def test_apply_and_balance():
    L = Ledger()
    rows = L.apply([("deposit","a",100),("withdraw","a",30),("deposit","b",5)], "a")
    assert rows == [("deposit","a",100),("withdraw","a",30)], rows
    assert balance(rows, "a") == 70
    assert balance(rows, "b") == 0
def test_apply_rejects_negative():
    L = Ledger()
    try:
        L.apply([("deposit","a",-1)], "a")
    except ValueError:
        pass
    else:
        raise AssertionError("negative accepted")
