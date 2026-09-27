def evaluate(rules, facts):
    """Evaluate a rule against a fact mapping.

    Supported rule shapes:
      {"all": [name, ...]}  -> True only if every fact is truthy
      {"any": [name, ...]}  -> True if at least one fact is truthy

    Returns a real bool so callers may use `is True` / `is False`.
    """
    if not isinstance(rules, dict):
        raise TypeError("rules must be a dict")

    if "all" in rules:
        names = rules["all"]
        return all(bool(facts.get(name, False)) for name in names)

    if "any" in rules:
        names = rules["any"]
        return any(bool(facts.get(name, False)) for name in names)

    raise ValueError("rules must contain 'all' or 'any'")
