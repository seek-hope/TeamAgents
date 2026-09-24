def evaluate(rules, facts):
    """Evaluate a rule set against a fact mapping.

    Supports ``{"all": [...]}`` (every named fact must be truthy) and
    ``{"any": [...]}`` (at least one named fact must be truthy). Missing
    facts are treated as falsy. An empty list returns the natural identity
    for the operator (``all([])`` -> True, ``any([])`` -> False).
    """
    if "all" in rules:
        return all(bool(facts.get(condition)) for condition in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(condition)) for condition in rules["any"])
    return False
