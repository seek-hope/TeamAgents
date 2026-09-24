def evaluate(rules, facts):
    """Evaluate a rule against a set of boolean facts.

    Supported rule shapes:
      - ``{"all": [...]}``: true when every named fact is truthy.
      - ``{"any": [...]}``: true when at least one named fact is truthy.

    Missing facts are treated as ``False``.  A result is always a real bool.
    """
    if "all" in rules:
        return all(bool(facts.get(name, False)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name, False)) for name in rules["any"])
    return False
