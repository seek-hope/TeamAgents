def evaluate(rules, facts):
    """Evaluate a rule against ``facts``.

    Supported rule forms:
      * ``{"all": [key, ...]}`` -> True only if every listed fact is truthy.
      * ``{"any": [key, ...]}`` -> True if at least one listed fact is truthy.

    Returns a real ``bool`` so callers can rely on identity comparisons.
    """
    if not isinstance(rules, dict):
        raise TypeError("rules must be a dict")

    if "all" in rules:
        return bool(all(bool(facts.get(key)) for key in rules["all"]))
    if "any" in rules:
        return bool(any(bool(facts.get(key)) for key in rules["any"]))

    raise ValueError("rules must contain either 'all' or 'any'")
