def evaluate(rules, facts):
    """Evaluate a rule against facts.

    ``{"all": [...]}`` is true when every referenced fact is truthy.
    ``{"any": [...]}`` is true when at least one referenced fact is truthy.
    Missing facts are treated as false.
    """
    if "all" in rules:
        return all(bool(facts.get(name, False)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name, False)) for name in rules["any"])
    return False
