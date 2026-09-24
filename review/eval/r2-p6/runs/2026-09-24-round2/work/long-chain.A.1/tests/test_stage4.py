from stage4 import report
def test_report():
    assert report({10:2, 7:4}) == "7:4\n10:2\n"
