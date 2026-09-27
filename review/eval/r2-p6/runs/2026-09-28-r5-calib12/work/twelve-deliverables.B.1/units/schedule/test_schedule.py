from schedule import slots, overlaps
def test_slots_merges_and_splits():
    assert slots([(0,60),(30,90),(120,150)], 30) == [(0,90),(120,150)]
    assert slots([(0,10)], 5) == [(0,10)]
def test_overlaps():
    assert overlaps((0,10),(5,20)) is True
    assert overlaps((0,10),(10,20)) is False
