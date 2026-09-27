def evaluate(rules, facts):
    """Evaluate a rule against named facts.

    Supported rule forms:
      {"all": [name, ...]} -> True when every named fact is truthy
      {"any": [name, ...]} -> True when at least one named fact is truthy

    A name that is absent from ``facts`` is treated as falsy.
    Returns a real ``bool`` (tests use ``is True`` / ``is False``).
    """
    facts = facts or {}

    if "all" in rules:
        return bool(all(facts.get(name) for name in rules["all"]))
    if "any" in rules:
        return bool(any(facts.get(name) for name in rules["any"]))
    return False
