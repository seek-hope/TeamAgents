def evaluate(rules, facts):
    """Evaluate a rule against a facts mapping and return a bool.

    Supported rule shapes:

    * ``{"all": [name, ...]}`` -> ``True`` iff every named fact is truthy.
    * ``{"any": [name, ...]}`` -> ``True`` iff at least one named fact is truthy.

    A fact name that is absent from ``facts`` counts as falsy.
    """
    if "all" in rules:
        return all(bool(facts.get(name)) for name in rules["all"])
    if "any" in rules:
        return any(bool(facts.get(name)) for name in rules["any"])
    raise ValueError("rule must contain 'all' or 'any': %r" % (rules,))
