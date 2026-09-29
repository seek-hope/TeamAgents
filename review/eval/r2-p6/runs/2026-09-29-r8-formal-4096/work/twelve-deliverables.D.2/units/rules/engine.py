def evaluate(rules, facts):
    """Evaluate a rule mapping against a mapping of facts.

    Supported rule forms:
      {"all": ["a", "b"]} -> True only if every named fact is truthy
      {"any": ["a", "b"]} -> True if at least one named fact is truthy

    Missing facts count as False. An empty "all" is True (vacuous truth),
    an empty "any" is False.
    """
    if not isinstance(rules, dict):
        raise TypeError("rules must be a dict")

    keys = [k for k in rules if k in ("all", "any")]
    if not keys:
        raise ValueError("rules must contain 'all' or 'any'")

    result = None
    for key in keys:
        names = rules[key]
        values = [bool(facts.get(name, False)) for name in names]
        if key == "all":
            current = all(values)
        else:
            current = any(values)
        # A rule mapping carrying several combinators requires all of them.
        result = current if result is None else (result and current)

    return bool(result)
