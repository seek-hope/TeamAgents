def evaluate(rules, facts):
    """Evaluate a rule mapping against a fact mapping.

    Supported rules:
      {"all": [<keys>]} -> True iff every listed key maps to a truthy fact.
      {"any": [<keys>]} -> True iff at least one listed key maps to a truthy fact.

    Missing keys count as falsy. Empty lists follow the standard conventions:
    all([]) is True, any([]) is False.
    """
    if not isinstance(rules, dict):
        raise ValueError("rules must be a mapping")

    for op, keys in rules.items():
        if op == "all":
            return all(facts.get(key) for key in keys)
        if op == "any":
            return any(facts.get(key) for key in keys)
        raise ValueError("unknown rule operator: %r" % (op,))

    # No rules: nothing to fail.
    return True
