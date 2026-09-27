def evaluate(rules, facts):
    """Evaluate a rule against ``facts``.

    ``{"all": [k, ...]}`` is true when every key is a truthy fact;
    ``{"any": [k, ...]}`` is true when at least one key is truthy.
    A missing key counts as false.  Unknown rule shapes return False.
    """
    if "all" in rules:
        return all(bool(facts.get(key, False)) for key in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(key, False)) for key in rules["any"])
    return False
