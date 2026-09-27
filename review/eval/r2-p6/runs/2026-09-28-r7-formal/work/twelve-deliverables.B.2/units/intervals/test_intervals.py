from intervals import Impl

I = Impl()

def test_merge_orders_joins_and_keeps_real_gaps():
    # (-1,0) 与 (1,3) 之间没有缺失整数（相邻），(3,5) 与 (5,7) 之间缺 1 个
    assert I.merge([[5, 7], [1, 3], [-1, 0]]) == [(-1, 3), (5, 7)]
    assert I.merge([]) == []
    assert I.merge([[1, 1]]) == [(1, 1)]

def test_merge_gap_counts_missing_integers():
    assert I.merge([[1, 2], [4, 5]]) == [(1, 2), (4, 5)]
    assert I.merge([[1, 2], [4, 5]], 1) == [(1, 5)]
    assert I.merge([[1, 2], [3, 4]]) == [(1, 4)]
    assert I.merge([[10, 12], [0, 1], [5, 6]], 3) == [(0, 12)]

def test_merge_ignores_nested_and_backwards():
    assert I.merge([[0, 10], [2, 3]]) == [(0, 10)]
    assert I.merge([[7, 9], [0, 1]]) == [(0, 1), (7, 9)]

def test_subtract_splits_inside():
    assert I.subtract([[0, 10]], (4, 6)) == [(0, 3), (7, 10)]

def test_subtract_covers_and_touches():
    assert I.subtract([[0, 10]], (0, 10)) == []
    assert I.subtract([[0, 4]], (4, 9)) == [(0, 3)]
    assert I.subtract([[0, 4], [8, 9]], (5, 7)) == [(0, 4), (8, 9)]

def test_subtract_normalizes_its_input_first():
    assert I.subtract([[5, 7], [1, 3]], (2, 6)) == [(1, 1), (7, 7)]

def test_total_length_counts_each_integer_once():
    assert I.total_length([[0, 2], [2, 4]]) == 5
    assert I.total_length([]) == 0
    assert I.total_length([[3, 3]]) == 1
