import impl

def test_parse_trims_and_validates():
    assert impl.parse(' a , 2 , x ') == ["a", "2", "x"]
    assert impl.parse('a,2') is None  # 列数不是 3 时返回 None

def test_total_skips_blank_and_reports_bad():
    rows = [["a", "1", "2"], ["b", "2", ""]]
    assert impl.total(rows) == 2
