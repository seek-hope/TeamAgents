import engine

def test_all_and_any():
    facts = {"a": True, "b": False}
    assert engine.evaluate({"all": ["a"]}, facts) is True
    assert engine.evaluate({"all": ["a", "b"]}, facts) is False
    assert engine.evaluate({"any": ["b", "a"]}, facts) is True
    assert engine.evaluate({"any": ["b"]}, facts) is False
