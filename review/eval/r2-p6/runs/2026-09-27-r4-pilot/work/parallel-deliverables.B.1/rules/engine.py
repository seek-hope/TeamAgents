def evaluate(rules, facts):
    """Evaluate a rule against a mapping of facts to truth values.

    ``{"all": [names...]}`` is True when every named fact is truthy.
    ``{"any": [names...]}`` is True when at least one named fact is truthy.
    Unknown facts are treated as falsy, and an unrecognised rule is False.
    """
    if "all" in rules:
        return all(bool(facts.get(name)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name)) for name in rules["any"])
    return False
