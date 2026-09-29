from flags import Impl

I = Impl()

def test_equals_and_space_forms_agree():
    assert I.parse(["--mode=fast", "-v"]) == {"values": {"mode": ["fast"]}, "flags": {"v": True}, "positional": []}
    assert I.parse(["--mode", "fast", "-v"]) == {"values": {"mode": ["fast"]}, "flags": {"v": True}, "positional": []}

def test_switch_when_no_value_follows():
    assert I.parse(["--mode", "--other"]) == {"values": {}, "flags": {"mode": True, "other": True}, "positional": []}
    assert I.parse(["--mode"]) == {"values": {}, "flags": {"mode": True}, "positional": []}

def test_bundled_short_switches():
    assert I.parse(["-abc"]) == {"values": {}, "flags": {"a": True, "b": True, "c": True}, "positional": []}

def test_repeated_key_accumulates_in_order():
    assert I.parse(["--k", "1", "--k", "2"])["values"] == {"k": ["1", "2"]}

def test_repeated_switch_stays_true():
    assert I.parse(["--x", "y", "--x"]) == {"values": {"x": ["y"]}, "flags": {"x": True}, "positional": []}

def test_positional_and_dash_terminator():
    assert I.parse(["a", "-", "b"])["positional"] == ["a", "-", "b"]
    assert I.parse(["--", "--not-a-flag", "x"])["positional"] == ["--not-a-flag", "x"]
    assert I.parse(["--", "--not-a-flag", "x"])["flags"] == {}

def test_values_may_look_like_switches():
    assert I.parse(["--k", "-5"])["values"] == {"k": ["-5"]}
    assert I.parse(["--k=--v"])["values"] == {"k": ["--v"]}

def test_empty_input():
    assert I.parse([]) == {"values": {}, "flags": {}, "positional": []}
