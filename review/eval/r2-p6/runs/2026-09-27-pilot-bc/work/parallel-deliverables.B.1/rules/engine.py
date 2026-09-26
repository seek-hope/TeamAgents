def evaluate(rules, facts):
    """Evaluate a rule expression against a facts mapping.

    Supported rule shapes:
      * ``{"all": [fact, ...]}`` -> True when every listed fact is truthy.
      * ``{"any": [fact, ...]}`` -> True when at least one listed fact is truthy.
    """
    if "all" in rules:
        return all(bool(facts.get(name)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name)) for name in rules["any"])
    return False
