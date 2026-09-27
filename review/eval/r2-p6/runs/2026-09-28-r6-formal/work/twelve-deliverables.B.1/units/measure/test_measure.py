from measure import Impl
I = Impl()
def test_cm():
    assert I.to_cm(1.5, "m") == 150
    assert I.to_cm(3, "cm") == 3
    assert I.to_cm(20, "mm") == 2
    try:
        I.to_cm(1, "ft")
    except ValueError:
        pass
    else:
        raise AssertionError("bad unit accepted")
