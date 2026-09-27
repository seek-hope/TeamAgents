from ranges import Impl
I = Impl()
def test_merge():
    assert I.merge([[5,7],[1,3],[2,4]]) == [(1,4),(5,7)]
    assert I.merge([]) == []
    assert I.merge([[1,1]]) == [(1,1)]
