def evaluate(rules, facts):
    """Evaluate a rule dict against a facts dict.

    Supported rule forms:
      {"all": [names...]} -> True iff every named fact is truthy.
                             An empty list is vacuously True.
      {"any": [names...]} -> True iff at least one named fact is truthy.
                             An empty list is False.

    Facts are looked up in ``facts``; a missing name counts as falsy.
    Always returns a real ``bool`` (not just a truthy/falsy value).
    """
    if not isinstance(rules, dict):
        raise TypeError("rules must be a dict")

    if "all" in rules:
        names = rules["all"]
        return all(bool(facts.get(name)) for name in names)

    if "any" in rules:
        names = rules["any"]
        return any(bool(facts.get(name)) for name in names)

    raise ValueError("rule must contain 'all' or 'any'")
