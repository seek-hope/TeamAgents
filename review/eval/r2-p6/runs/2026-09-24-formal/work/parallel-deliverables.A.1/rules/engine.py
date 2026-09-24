def evaluate(rules, facts):
    """Evaluate a rule against boolean ``facts``.

    Supports ``{"all": [...]}`` (every referenced fact is truthy) and
    ``{"any": [...]}`` (at least one referenced fact is truthy). An unknown
    fact name is treated as ``False``.
    """
    if "all" in rules:
        return all(bool(facts.get(name, False)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name, False)) for name in rules["any"])
    return False
